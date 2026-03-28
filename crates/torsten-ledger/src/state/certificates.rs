use super::{credential_to_hash, DRepRegistration, LedgerState, PoolRegistration};
use std::sync::Arc;
use torsten_primitives::credentials::Credential;
use torsten_primitives::hash::Hash32;
use torsten_primitives::transaction::{Certificate, MIRSource, MIRTarget};
use torsten_primitives::value::Lovelace;
use tracing::debug;

/// Returns true if the certificate is Conway-only and requires protocol version >= 9.
#[allow(dead_code)]
pub(crate) fn is_conway_only_certificate(cert: &Certificate) -> bool {
    matches!(
        cert,
        Certificate::RegDRep { .. }
            | Certificate::UnregDRep { .. }
            | Certificate::UpdateDRep { .. }
            | Certificate::VoteDelegation { .. }
            | Certificate::StakeVoteDelegation { .. }
            | Certificate::CommitteeHotAuth { .. }
            | Certificate::CommitteeColdResign { .. }
            | Certificate::RegStakeVoteDeleg { .. }
            | Certificate::VoteRegDeleg { .. }
            | Certificate::ConwayStakeRegistration { .. }
            | Certificate::ConwayStakeDeregistration { .. }
            | Certificate::RegStakeDeleg { .. }
    )
}

impl LedgerState {
    /// Process a certificate with pointer tracking for Pointer address resolution.
    ///
    /// StakeRegistration certificates create entries in the pointer_map,
    /// mapping (slot, tx_index, cert_index) → credential hash. This enables
    /// resolution of Pointer addresses (type 4/5) in stake_credential_hash.
    pub(crate) fn process_certificate_with_pointer(
        &mut self,
        cert: &Certificate,
        slot: u64,
        tx_index: u64,
        cert_index: u64,
    ) {
        // Populate pointer_map for StakeRegistration certificates
        if let Certificate::StakeRegistration(credential)
        | Certificate::ConwayStakeRegistration {
            credential,
            deposit: _,
        } = cert
        {
            let key = credential_to_hash(credential);
            let pointer = torsten_primitives::credentials::Pointer {
                slot,
                tx_index,
                cert_index,
            };
            self.pointer_map.insert(pointer, key);
        }
        // Also handle combined registration certificates
        if let Certificate::RegStakeDeleg {
            credential,
            pool_hash: _,
            ..
        }
        | Certificate::RegStakeVoteDeleg {
            credential,
            pool_hash: _,
            drep: _,
            ..
        }
        | Certificate::VoteRegDeleg {
            credential,
            drep: _,
            ..
        } = cert
        {
            let key = credential_to_hash(credential);
            let pointer = torsten_primitives::credentials::Pointer {
                slot,
                tx_index,
                cert_index,
            };
            self.pointer_map.insert(pointer, key);
        }

        // Delegate to the existing process_certificate for the actual state updates
        self.process_certificate(cert);
    }

    /// Process a certificate and update the ledger state accordingly.
    ///
    /// Certificates are applied unconditionally during block application.
    /// Era-gating (e.g., Conway-only certs in pre-Conway era) is a Phase-1
    /// tx validation rule, not a block application rule. The block producer
    /// already validated era compatibility. During replay, the in-state
    /// protocol version may lag behind the block's actual era.
    pub(crate) fn process_certificate(&mut self, cert: &Certificate) {
        match cert {
            Certificate::StakeRegistration(credential) => {
                let key = credential_to_hash(credential);
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                // Track script credentials so N2C query responses can set credential_type correctly.
                if matches!(credential, Credential::Script(_)) {
                    self.script_stake_credentials.insert(key);
                }
                debug!("Stake key registered: {}", key.to_hex());
            }
            Certificate::StakeDeregistration(credential) => {
                let key = credential_to_hash(credential);
                // Do NOT remove from stake_distribution.stake_map — the credential
                // may still have UTxOs. The stake_map is a UTxO accounting structure;
                // deregistration is a delegation-layer concept. The ground truth
                // (rebuild_stake_distribution) sums ALL UTxOs by credential regardless
                // of registration status.
                Arc::make_mut(&mut self.delegations).remove(&key);
                Arc::make_mut(&mut self.reward_accounts).remove(&key);
                self.script_stake_credentials.remove(&key);
                // Remove pointer entries for this credential
                self.pointer_map.retain(|_, v| *v != key);
                debug!("Stake key deregistered: {}", key.to_hex());
            }
            Certificate::ConwayStakeRegistration {
                credential,
                deposit: _,
            } => {
                // Conway cert tag 7: same behavior as StakeRegistration
                let key = credential_to_hash(credential);
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                if matches!(credential, Credential::Script(_)) {
                    self.script_stake_credentials.insert(key);
                }
                debug!("Stake key registered (Conway): {}", key.to_hex());
            }
            Certificate::ConwayStakeDeregistration {
                credential,
                refund: _,
            } => {
                // Conway cert tag 8: deregistration returns remaining reward balance
                // as part of the deposit refund. Remove from delegations/rewards but
                // keep the stake_map entry — UTxOs may still exist at this credential.
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.delegations).remove(&key);
                Arc::make_mut(&mut self.reward_accounts).remove(&key);
                self.script_stake_credentials.remove(&key);
                debug!("Stake key deregistered (Conway): {}", key.to_hex());
            }
            Certificate::StakeDelegation {
                credential,
                pool_hash,
            } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                debug!("Stake delegated to pool: {}", pool_hash.to_hex());
            }
            Certificate::PoolRegistration(params) => {
                let pool_reg = PoolRegistration {
                    pool_id: params.operator,
                    vrf_keyhash: params.vrf_keyhash,
                    pledge: params.pledge,
                    cost: params.cost,
                    margin_numerator: params.margin.numerator,
                    margin_denominator: params.margin.denominator,
                    reward_account: params.reward_account.clone(),
                    owners: params.pool_owners.clone(),
                    relays: params.relays.clone(),
                    metadata_url: params.pool_metadata.as_ref().map(|m| m.url.clone()),
                    metadata_hash: params.pool_metadata.as_ref().map(|m| m.hash),
                };
                // If the pool is re-registering, cancel any pending retirement
                // and store new params in future_pool_params (applied at next epoch
                // boundary, matching Haskell's POOL STS futurePoolParams mechanism).
                // First registrations go directly to pool_params.
                if self.pool_params.contains_key(&params.operator) {
                    // Cancel any pending retirement (matching Haskell's
                    // psRetiringL %~ Map.delete sppId).
                    self.pending_retirements.remove(&params.operator);
                    // Re-registration: defer to future_pool_params
                    self.future_pool_params.insert(params.operator, pool_reg);
                    debug!(
                        "Pool re-registered (deferred to next epoch, pending retirement cancelled): {}",
                        params.operator.to_hex()
                    );
                } else {
                    // First registration: apply immediately
                    Arc::make_mut(&mut self.pool_params).insert(params.operator, pool_reg);
                    debug!("Pool registered: {}", params.operator.to_hex());
                }
            }
            Certificate::PoolRetirement { pool_hash, epoch } => {
                // Apply the retirement unconditionally. The e_max check
                // (retirement_epoch <= current_epoch + e_max) is a Phase-1
                // transaction validation rule, NOT a block application rule.
                // Blocks already on-chain have passed validation — re-checking
                // during replay with the wrong "current epoch" causes false
                // rejections and ledger state divergence.
                debug!(
                    "Pool retirement scheduled at epoch {}: {}",
                    epoch,
                    pool_hash.to_hex()
                );
                // Insert or replace the retirement epoch for this pool.
                // Haskell: psRetiringL %~ Map.insert sppId epoch
                // A second retirement for the same pool replaces the first.
                self.pending_retirements
                    .insert(*pool_hash, torsten_primitives::time::EpochNo(*epoch));
            }
            Certificate::RegStakeDeleg {
                credential,
                pool_hash,
                ..
            } => {
                let key = credential_to_hash(credential);
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                if matches!(credential, Credential::Script(_)) {
                    self.script_stake_credentials.insert(key);
                }
            }
            Certificate::RegDRep {
                credential,
                deposit,
                anchor,
            } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.governance).dreps.insert(
                    key,
                    DRepRegistration {
                        credential: credential.clone(),
                        deposit: *deposit,
                        anchor: anchor.clone(),
                        registered_epoch: self.epoch,
                        last_active_epoch: self.epoch,
                        active: true,
                    },
                );
                Arc::make_mut(&mut self.governance).drep_registration_count += 1;
                debug!("DRep registered: {}", key.to_hex());
            }
            Certificate::UnregDRep { credential, refund } => {
                let key = credential_to_hash(credential);
                // Refund the DRep deposit to their reward account.
                // Per the Haskell ledger spec (Conway DELEG rule), the deposit
                // is returned to the credential's reward account upon
                // unregistration.  If a refund amount is specified in the
                // certificate it must match the recorded deposit (enforced by
                // validation); we use the recorded deposit when available.
                let deposit_amount = Arc::make_mut(&mut self.governance)
                    .dreps
                    .remove(&key)
                    .map(|reg| reg.deposit)
                    .unwrap_or(*refund);
                if deposit_amount.0 > 0 {
                    // Credit the deposit back to the credential's reward account.
                    // The reward account key is the same credential hash used for
                    // DRep registration (Hash32 of the credential).
                    *Arc::make_mut(&mut self.reward_accounts)
                        .entry(key)
                        .or_insert(Lovelace(0)) += deposit_amount;
                    debug!(
                        "DRep deregistered: {}, deposit {} refunded to reward account",
                        key.to_hex(),
                        deposit_amount.0
                    );
                } else {
                    debug!("DRep deregistered: {}", key.to_hex());
                }
            }
            Certificate::UpdateDRep { credential, anchor } => {
                let key = credential_to_hash(credential);
                if let Some(drep) = Arc::make_mut(&mut self.governance).dreps.get_mut(&key) {
                    drep.anchor = anchor.clone();
                    drep.last_active_epoch = self.epoch;
                    debug!("DRep updated: {}", key.to_hex());
                }
            }
            Certificate::VoteDelegation { credential, drep } => {
                let key = credential_to_hash(credential);
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                debug!("Vote delegated to {:?}", drep);
            }
            Certificate::StakeVoteDelegation {
                credential,
                pool_hash,
                drep,
            } => {
                let key = credential_to_hash(credential);
                // Stake delegation
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                // Vote delegation
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                debug!(
                    "Stake+vote delegated to pool {} and drep {:?}",
                    pool_hash.to_hex(),
                    drep
                );
            }
            Certificate::CommitteeHotAuth {
                cold_credential,
                hot_credential,
            } => {
                let cold_key = credential_to_hash(cold_credential);
                let hot_key = credential_to_hash(hot_credential);
                let gov = Arc::make_mut(&mut self.governance);
                gov.committee_hot_keys.insert(cold_key, hot_key);
                // Remove from resigned if re-authorizing
                gov.committee_resigned.remove(&cold_key);
                // Track script cold credentials for correct cold_credential_type in N2C responses.
                if matches!(cold_credential, Credential::Script(_)) {
                    gov.script_committee_credentials.insert(cold_key);
                }
                // Track script hot credentials for correct hot_credential_type in N2C responses
                // (GetCommitteeState tag 27).
                //
                // The set is keyed by hot credential hash.  When querying, we resolve the
                // current hot key for a cold key via committee_hot_keys, then probe this set.
                // Therefore stale entries from a superseded hot key can never be reached:
                // once committee_hot_keys[cold_key] points to a new hot key hash, the old
                // hash is simply never looked up again.  There is no need to remove the
                // displaced hash here.
                if matches!(hot_credential, Credential::Script(_)) {
                    gov.script_committee_hot_credentials.insert(hot_key);
                }
                debug!(
                    "Committee hot key authorized: {} -> {}",
                    cold_key.to_hex(),
                    hot_key.to_hex()
                );
            }
            Certificate::CommitteeColdResign {
                cold_credential,
                anchor,
            } => {
                let cold_key = credential_to_hash(cold_credential);
                let gov = Arc::make_mut(&mut self.governance);
                gov.committee_resigned.insert(cold_key, anchor.clone());
                gov.committee_hot_keys.remove(&cold_key);
                // Track script cold credentials for correct credential_type in N2C responses.
                if matches!(cold_credential, Credential::Script(_)) {
                    gov.script_committee_credentials.insert(cold_key);
                }
                debug!("Committee member resigned: {}", cold_key.to_hex());
            }
            Certificate::RegStakeVoteDeleg {
                credential,
                pool_hash,
                drep,
                ..
            } => {
                let key = credential_to_hash(credential);
                // Register stake credential
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                // Stake delegation
                Arc::make_mut(&mut self.delegations).insert(key, *pool_hash);
                // Vote delegation
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                if matches!(credential, Credential::Script(_)) {
                    self.script_stake_credentials.insert(key);
                }
                debug!(
                    "Reg+stake+vote delegated: pool={}, drep={:?}",
                    pool_hash.to_hex(),
                    drep
                );
            }
            Certificate::VoteRegDeleg {
                credential, drep, ..
            } => {
                let key = credential_to_hash(credential);
                // Register stake credential
                self.stake_distribution
                    .stake_map
                    .entry(key)
                    .or_insert(Lovelace(0));
                Arc::make_mut(&mut self.reward_accounts)
                    .entry(key)
                    .or_insert(Lovelace(0));
                // Vote delegation
                Arc::make_mut(&mut self.governance)
                    .vote_delegations
                    .insert(key, drep.clone());
                if matches!(credential, Credential::Script(_)) {
                    self.script_stake_credentials.insert(key);
                }
                debug!("Reg+vote delegated to {:?}", drep);
            }
            Certificate::GenesisKeyDelegation {
                genesis_hash,
                genesis_delegate_hash,
                vrf_keyhash,
            } => {
                // Genesis key delegation — update genesis delegate mapping
                // These are rare (Shelley-era governance by genesis keys)
                debug!(
                    "Genesis key delegation: {} -> delegate={}, vrf={}",
                    genesis_hash.to_hex(),
                    genesis_delegate_hash.to_hex(),
                    vrf_keyhash.to_hex()
                );
            }
            Certificate::MoveInstantaneousRewards { source, target } => {
                // MIR: transfer funds between reserves/treasury or distribute to stake credentials
                match target {
                    MIRTarget::StakeCredentials(creds) => {
                        let mut total_distributed: u64 = 0;
                        for (cred, amount) in creds {
                            let key = credential_to_hash(cred);
                            let entry = Arc::make_mut(&mut self.reward_accounts)
                                .entry(key)
                                .or_insert(Lovelace(0));
                            if *amount >= 0 {
                                let amt = *amount as u64;
                                entry.0 = entry.0.saturating_add(amt);
                                total_distributed = total_distributed.saturating_add(amt);
                            } else {
                                entry.0 = entry.0.saturating_sub(amount.unsigned_abs());
                            }
                            debug!(
                                "MIR: distributed {} lovelace from {:?} to {}",
                                amount,
                                source,
                                key.to_hex()
                            );
                        }
                        // Debit the source pot for the total positive amount distributed
                        if total_distributed > 0 {
                            match source {
                                MIRSource::Reserves => {
                                    self.reserves.0 =
                                        self.reserves.0.saturating_sub(total_distributed);
                                }
                                MIRSource::Treasury => {
                                    self.treasury.0 =
                                        self.treasury.0.saturating_sub(total_distributed);
                                }
                            }
                        }
                    }
                    MIRTarget::OtherAccountingPot(coin) => {
                        // Transfer between reserves and treasury
                        // Use saturating arithmetic to handle compound MIR operations
                        // where credential distributions and pot transfers interact
                        match source {
                            MIRSource::Reserves => {
                                // Move from reserves to treasury, capped at available
                                let actual = (*coin).min(self.reserves.0);
                                self.reserves.0 = self.reserves.0.saturating_sub(actual);
                                self.treasury.0 = self.treasury.0.saturating_add(actual);
                                debug!(
                                    "MIR: transferred {} lovelace from reserves to treasury",
                                    actual
                                );
                            }
                            MIRSource::Treasury => {
                                // Move from treasury to reserves, capped at available
                                let actual = (*coin).min(self.treasury.0);
                                self.treasury.0 = self.treasury.0.saturating_sub(actual);
                                self.reserves.0 = self.reserves.0.saturating_add(actual);
                                debug!(
                                    "MIR: transferred {} lovelace from treasury to reserves",
                                    actual
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Process a withdrawal from a reward account.
    /// Per Cardano spec, the withdrawal amount must exactly match the reward balance.
    /// After withdrawal, the balance is reduced by the withdrawal amount.
    pub(crate) fn process_withdrawal(&mut self, reward_account: &[u8], amount: Lovelace) {
        let key = Self::reward_account_to_hash(reward_account);
        if let Some(balance) = Arc::make_mut(&mut self.reward_accounts).get_mut(&key) {
            // Per Cardano spec, withdrawal amount must exactly equal the reward balance.
            // During sync from genesis, we may not have accumulated all rewards yet,
            // so we only warn and process as best-effort.
            if balance.0 != amount.0 {
                debug!(
                    account = %key.to_hex(),
                    balance = balance.0,
                    withdrawal = amount.0,
                    "Withdrawal amount does not match reward balance"
                );
            }
            // Always process the withdrawal: set balance to 0
            // (rewards were consumed in the on-chain transaction)
            balance.0 = 0;
        }
    }

    /// Convert a reward account (raw bytes with network header) to a Hash32 key.
    ///
    /// Reward addresses are 29 bytes: 1 byte network header + 28 byte credential hash.
    /// We extract exactly the 28-byte credential and zero-pad to 32 bytes for Hash32.
    pub fn reward_account_to_hash(reward_account: &[u8]) -> Hash32 {
        let mut key_bytes = [0u8; 32];
        if reward_account.len() >= 29 {
            // Copy exactly 28 bytes of the credential (skip the 1-byte header)
            key_bytes[..28].copy_from_slice(&reward_account[1..29]);
            // Encode credential type from the header byte:
            // Bit 4 of the header: 0 = key hash, 1 = script hash
            // Reward address headers: 0xe0/0xe1 = key, 0xf0/0xf1 = script
            if reward_account[0] & 0x10 != 0 {
                key_bytes[28] = 0x01; // script credential
            }
        }
        Hash32::from_bytes(key_bytes)
    }
}
