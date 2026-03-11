use crate::cbor::*;
use torsten_primitives::transaction::*;

use super::governance::{encode_drep, encode_optional_anchor};

/// Encode a credential [type, hash]
pub(crate) fn encode_credential(cred: &torsten_primitives::credentials::Credential) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match cred {
        torsten_primitives::credentials::Credential::VerificationKey(h) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_hash28(h));
        }
        torsten_primitives::credentials::Credential::Script(h) => {
            buf.extend(encode_uint(1));
            buf.extend(encode_hash28(h));
        }
    }
    buf
}

/// Encode an anchor [url, data_hash]
pub(crate) fn encode_anchor(anchor: &Anchor) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    buf.extend(encode_text(&anchor.url));
    buf.extend(encode_hash32(&anchor.data_hash));
    buf
}

/// Encode a Rational as CBOR tag 30 [numerator, denominator]
pub(crate) fn encode_rational(r: &Rational) -> Vec<u8> {
    let mut buf = encode_tag(30);
    buf.extend(encode_array_header(2));
    buf.extend(encode_uint(r.numerator));
    buf.extend(encode_uint(r.denominator));
    buf
}

/// Encode a relay
pub(crate) fn encode_relay(relay: &Relay) -> Vec<u8> {
    match relay {
        Relay::SingleHostAddr { port, ipv4, ipv6 } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(0));
            match port {
                Some(p) => buf.extend(encode_uint(*p as u64)),
                None => buf.extend(encode_null()),
            }
            match ipv4 {
                Some(ip) => buf.extend(encode_bytes(ip)),
                None => buf.extend(encode_null()),
            }
            match ipv6 {
                Some(ip) => buf.extend(encode_bytes(ip)),
                None => buf.extend(encode_null()),
            }
            buf
        }
        Relay::SingleHostName { port, dns_name } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(1));
            match port {
                Some(p) => buf.extend(encode_uint(*p as u64)),
                None => buf.extend(encode_null()),
            }
            buf.extend(encode_text(dns_name));
            buf
        }
        Relay::MultiHostName { dns_name } => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(2));
            buf.extend(encode_text(dns_name));
            buf
        }
    }
}

/// Encode pool parameters
pub(crate) fn encode_pool_params(params: &PoolParams) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend(encode_hash28(&params.operator));
    buf.extend(encode_hash32(&params.vrf_keyhash));
    buf.extend(encode_uint(params.pledge.0));
    buf.extend(encode_uint(params.cost.0));
    buf.extend(encode_rational(&params.margin));
    buf.extend(encode_bytes(&params.reward_account));

    // pool_owners as set
    buf.extend(encode_array_header(params.pool_owners.len()));
    for owner in &params.pool_owners {
        buf.extend(encode_hash28(owner));
    }

    // relays
    buf.extend(encode_array_header(params.relays.len()));
    for relay in &params.relays {
        buf.extend(encode_relay(relay));
    }

    // pool_metadata
    match &params.pool_metadata {
        Some(meta) => {
            buf.extend(encode_array_header(2));
            buf.extend(encode_text(&meta.url));
            buf.extend(encode_hash32(&meta.hash));
        }
        None => buf.extend(encode_null()),
    }

    buf
}

/// Encode a certificate
pub fn encode_certificate(cert: &Certificate) -> Vec<u8> {
    match cert {
        Certificate::StakeRegistration(cred) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(0));
            buf.extend(encode_credential(cred));
            buf
        }
        Certificate::StakeDeregistration(cred) => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(1));
            buf.extend(encode_credential(cred));
            buf
        }
        Certificate::ConwayStakeRegistration {
            credential,
            deposit,
        } => {
            // Conway cert tag 7: Reg
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(7));
            buf.extend(encode_credential(credential));
            buf.extend(encode_uint(deposit.0));
            buf
        }
        Certificate::ConwayStakeDeregistration { credential, refund } => {
            // Conway cert tag 8: UnReg
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(8));
            buf.extend(encode_credential(credential));
            buf.extend(encode_uint(refund.0));
            buf
        }
        Certificate::StakeDelegation {
            credential,
            pool_hash,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(2));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf
        }
        Certificate::PoolRegistration(params) => {
            let mut buf = encode_array_header(10);
            buf.extend(encode_uint(3));
            buf.extend(encode_pool_params(params));
            buf
        }
        Certificate::PoolRetirement { pool_hash, epoch } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(4));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_uint(*epoch));
            buf
        }
        Certificate::RegDRep {
            credential,
            deposit,
            anchor,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(16));
            buf.extend(encode_credential(credential));
            buf.extend(encode_uint(deposit.0));
            buf.extend(encode_optional_anchor(anchor));
            buf
        }
        Certificate::UnregDRep { credential, refund } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(17));
            buf.extend(encode_credential(credential));
            buf.extend(encode_uint(refund.0));
            buf
        }
        Certificate::UpdateDRep { credential, anchor } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(18));
            buf.extend(encode_credential(credential));
            buf.extend(encode_optional_anchor(anchor));
            buf
        }
        Certificate::VoteDelegation { credential, drep } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(9));
            buf.extend(encode_credential(credential));
            buf.extend(encode_drep(drep));
            buf
        }
        Certificate::StakeVoteDelegation {
            credential,
            pool_hash,
            drep,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(10));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_drep(drep));
            buf
        }
        Certificate::RegStakeDeleg {
            credential,
            pool_hash,
            deposit,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(11));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_uint(deposit.0));
            buf
        }
        Certificate::CommitteeHotAuth {
            cold_credential,
            hot_credential,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(14));
            buf.extend(encode_credential(cold_credential));
            buf.extend(encode_credential(hot_credential));
            buf
        }
        Certificate::CommitteeColdResign {
            cold_credential,
            anchor,
        } => {
            let mut buf = encode_array_header(3);
            buf.extend(encode_uint(15));
            buf.extend(encode_credential(cold_credential));
            buf.extend(encode_optional_anchor(anchor));
            buf
        }
        Certificate::RegStakeVoteDeleg {
            credential,
            pool_hash,
            drep,
            deposit,
        } => {
            let mut buf = encode_array_header(5);
            buf.extend(encode_uint(13));
            buf.extend(encode_credential(credential));
            buf.extend(encode_hash28(pool_hash));
            buf.extend(encode_drep(drep));
            buf.extend(encode_uint(deposit.0));
            buf
        }
        Certificate::VoteRegDeleg {
            credential,
            drep,
            deposit,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(12));
            buf.extend(encode_credential(credential));
            buf.extend(encode_drep(drep));
            buf.extend(encode_uint(deposit.0));
            buf
        }
        Certificate::GenesisKeyDelegation {
            genesis_hash,
            genesis_delegate_hash,
            vrf_keyhash,
        } => {
            let mut buf = encode_array_header(4);
            buf.extend(encode_uint(5));
            buf.extend(encode_hash32(genesis_hash));
            buf.extend(encode_hash32(genesis_delegate_hash));
            buf.extend(encode_hash32(vrf_keyhash));
            buf
        }
        Certificate::MoveInstantaneousRewards { source, target } => {
            let mut buf = encode_array_header(2);
            buf.extend(encode_uint(6));
            // MIR body: [source, target]
            let mut mir_buf = encode_array_header(2);
            mir_buf.extend(encode_uint(match source {
                MIRSource::Reserves => 0,
                MIRSource::Treasury => 1,
            }));
            match target {
                MIRTarget::StakeCredentials(creds) => {
                    mir_buf.extend(encode_map_header(creds.len()));
                    for (cred, amount) in creds {
                        mir_buf.extend(encode_credential(cred));
                        mir_buf.extend(encode_int(*amount as i128));
                    }
                }
                MIRTarget::OtherAccountingPot(coin) => {
                    mir_buf.extend(encode_uint(*coin));
                }
            }
            buf.extend(mir_buf);
            buf
        }
    }
}
