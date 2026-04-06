use crate::cbor::*;
use dugite_primitives::transaction::*;

use super::governance::{encode_drep, encode_optional_anchor};

/// Encode a credential [type, hash]
pub(crate) fn encode_credential(cred: &dugite_primitives::credentials::Credential) -> Vec<u8> {
    let mut buf = encode_array_header(2);
    match cred {
        dugite_primitives::credentials::Credential::VerificationKey(h) => {
            buf.extend(encode_uint(0));
            buf.extend(encode_hash28(h));
        }
        dugite_primitives::credentials::Credential::Script(h) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use dugite_primitives::{
        credentials::Credential,
        hash::{Hash28, Hash32},
        transaction::{
            Anchor, Certificate, DRep, MIRSource, MIRTarget, PoolMetadata, PoolParams, Rational,
            Relay,
        },
        value::Lovelace,
    };

    // ── helpers ──────────────────────────────────────────────────────────────

    fn zero28() -> Hash28 {
        Hash28::ZERO
    }

    fn zero32() -> Hash32 {
        Hash32::ZERO
    }

    fn key_cred() -> Credential {
        Credential::VerificationKey(zero28())
    }

    fn script_cred() -> Credential {
        Credential::Script(Hash28::from_bytes([0x02; 28]))
    }

    fn make_pool_params() -> PoolParams {
        PoolParams {
            operator: zero28(),
            vrf_keyhash: zero32(),
            pledge: Lovelace(500_000_000),
            cost: Lovelace(340_000_000),
            margin: Rational {
                numerator: 1,
                denominator: 20,
            },
            reward_account: vec![0xe0; 29],
            pool_owners: vec![zero28()],
            relays: vec![],
            pool_metadata: None,
        }
    }

    // ── encode_credential ────────────────────────────────────────────────────

    /// VerificationKey encodes as array(2) [0, bstr(28)]
    #[test]
    fn test_encode_credential_key() {
        let encoded = encode_credential(&key_cred());
        // array(2) = 0x82, uint(0) = 0x00, then bstr(28)
        assert_eq!(encoded[0], 0x82, "array(2) header");
        assert_eq!(encoded[1], 0x00, "type tag 0 for VerificationKey");
        assert_eq!(encoded[2], 0x58, "bstr 1-byte length prefix");
        assert_eq!(encoded[3], 28, "hash length 28");
        assert_eq!(encoded.len(), 2 + 2 + 28, "total length");
    }

    /// Script encodes as array(2) [1, bstr(28)]
    #[test]
    fn test_encode_credential_script() {
        let encoded = encode_credential(&script_cred());
        assert_eq!(encoded[0], 0x82, "array(2) header");
        assert_eq!(encoded[1], 0x01, "type tag 1 for Script");
        assert_eq!(encoded[2], 0x58, "bstr 1-byte length prefix");
        assert_eq!(encoded[3], 28, "hash length 28");
        // hash bytes should all be 0x02
        assert!(encoded[4..].iter().all(|&b| b == 0x02));
    }

    // ── encode_anchor ────────────────────────────────────────────────────────

    /// Anchor encodes as array(2) [url, bstr(32)]
    #[test]
    fn test_encode_anchor_structure() {
        let anchor = Anchor {
            url: "https://example.com".to_string(),
            data_hash: zero32(),
        };
        let encoded = encode_anchor(&anchor);
        // array(2)
        assert_eq!(encoded[0], 0x82, "array(2) header");
        // text string prefix 0x73 = 0x60 | 19 (len of "https://example.com")
        assert_eq!(encoded[1], 0x60 | 19, "tstr header for 19-char url");
        // after url (1 + 19 = 20 bytes), hash32 starts
        let hash_start = 1 + 1 + 19; // array header + tstr header + url bytes
        assert_eq!(encoded[hash_start], 0x58, "bstr length-prefix for hash32");
        assert_eq!(encoded[hash_start + 1], 32, "hash32 length");
        assert_eq!(encoded.len(), 1 + 1 + 19 + 2 + 32);
    }

    // ── encode_rational ──────────────────────────────────────────────────────

    /// Rational encodes as tag(30) array(2) [numerator, denominator]
    #[test]
    fn test_encode_rational_tag_and_structure() {
        let r = Rational {
            numerator: 1,
            denominator: 20,
        };
        let encoded = encode_rational(&r);
        // tag(30) = 0xd8 0x1e
        assert_eq!(encoded[0], 0xd8, "CBOR tag major type");
        assert_eq!(encoded[1], 30, "tag value 30");
        // array(2)
        assert_eq!(encoded[2], 0x82, "array(2)");
        // numerator = 1
        assert_eq!(encoded[3], 0x01, "numerator = 1");
        // denominator = 20 = 0x14
        assert_eq!(encoded[4], 0x14, "denominator = 20");
    }

    #[test]
    fn test_encode_rational_large_values() {
        let r = Rational {
            numerator: 3,
            denominator: 100,
        };
        let encoded = encode_rational(&r);
        assert_eq!(encoded[0], 0xd8);
        assert_eq!(encoded[1], 30);
        assert_eq!(encoded[2], 0x82);
        // 3 fits in direct uint
        assert_eq!(encoded[3], 0x03);
        // 100 = 0x18 0x64
        assert_eq!(encoded[4], 0x18);
        assert_eq!(encoded[5], 100);
    }

    // ── encode_relay ─────────────────────────────────────────────────────────

    /// SingleHostAddr with all fields — array(4) [0, port, ipv4, ipv6]
    #[test]
    fn test_encode_relay_single_host_addr_all_fields() {
        let relay = Relay::SingleHostAddr {
            port: Some(3001),
            ipv4: Some([127, 0, 0, 1]),
            ipv6: None,
        };
        let encoded = encode_relay(&relay);
        // array(4)
        assert_eq!(encoded[0], 0x84, "array(4) header");
        assert_eq!(encoded[1], 0x00, "type tag 0");
        // port 3001 = 0x19 0x0b 0xb9
        assert_eq!(encoded[2], 0x19);
        let port = u16::from_be_bytes([encoded[3], encoded[4]]);
        assert_eq!(port, 3001);
        // ipv4 bstr(4) = 0x44 then 127 0 0 1
        assert_eq!(encoded[5], 0x44, "bstr(4) for ipv4");
        assert_eq!(&encoded[6..10], &[127, 0, 0, 1]);
        // ipv6 = null = 0xf6
        assert_eq!(encoded[10], 0xf6, "null for absent ipv6");
    }

    /// SingleHostAddr with no fields — all optional fields null
    #[test]
    fn test_encode_relay_single_host_addr_no_fields() {
        let relay = Relay::SingleHostAddr {
            port: None,
            ipv4: None,
            ipv6: None,
        };
        let encoded = encode_relay(&relay);
        assert_eq!(encoded[0], 0x84, "array(4)");
        assert_eq!(encoded[1], 0x00, "type 0");
        assert_eq!(encoded[2], 0xf6, "null port");
        assert_eq!(encoded[3], 0xf6, "null ipv4");
        assert_eq!(encoded[4], 0xf6, "null ipv6");
        assert_eq!(encoded.len(), 5);
    }

    /// SingleHostName — array(3) [1, port_or_null, dns_name]
    #[test]
    fn test_encode_relay_single_host_name_with_port() {
        let relay = Relay::SingleHostName {
            port: Some(6000),
            dns_name: "relay.example.com".to_string(),
        };
        let encoded = encode_relay(&relay);
        assert_eq!(encoded[0], 0x83, "array(3) header");
        assert_eq!(encoded[1], 0x01, "type tag 1");
        // port 6000 = 0x19 0x17 0x70
        assert_eq!(encoded[2], 0x19);
        let port = u16::from_be_bytes([encoded[3], encoded[4]]);
        assert_eq!(port, 6000);
        // dns_name as tstr
        let dns = "relay.example.com";
        assert_eq!(encoded[5], 0x60 | dns.len() as u8);
        assert_eq!(&encoded[6..6 + dns.len()], dns.as_bytes());
    }

    #[test]
    fn test_encode_relay_single_host_name_no_port() {
        let relay = Relay::SingleHostName {
            port: None,
            dns_name: "relay.pool.io".to_string(),
        };
        let encoded = encode_relay(&relay);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x01, "type 1");
        assert_eq!(encoded[2], 0xf6, "null port");
    }

    /// MultiHostName — array(2) [2, dns_name]
    #[test]
    fn test_encode_relay_multi_host_name() {
        let relay = Relay::MultiHostName {
            dns_name: "pool.example.com".to_string(),
        };
        let encoded = encode_relay(&relay);
        assert_eq!(encoded[0], 0x82, "array(2) header");
        assert_eq!(encoded[1], 0x02, "type tag 2");
        let dns = "pool.example.com";
        assert_eq!(encoded[2], 0x60 | dns.len() as u8);
        assert_eq!(&encoded[3..3 + dns.len()], dns.as_bytes());
        assert_eq!(encoded.len(), 3 + dns.len());
    }

    // ── encode_pool_params ───────────────────────────────────────────────────

    /// Pool params encodes operator hash, vrf_keyhash, pledge, cost, margin,
    /// reward_account, owners, relays, and metadata (or null).
    #[test]
    fn test_encode_pool_params_basic_structure() {
        let params = make_pool_params();
        let encoded = encode_pool_params(&params);

        // Starts with operator bstr(28) — 0x58 0x1c
        assert_eq!(encoded[0], 0x58, "operator bstr prefix");
        assert_eq!(encoded[1], 28, "operator length");

        // After operator (30 bytes), vrf_keyhash bstr(32) — 0x58 0x20
        let pos = 30; // 2 + 28
        assert_eq!(encoded[pos], 0x58, "vrf_keyhash bstr prefix");
        assert_eq!(encoded[pos + 1], 32, "vrf_keyhash length");

        // Not empty
        assert!(!encoded.is_empty());
    }

    #[test]
    fn test_encode_pool_params_null_metadata() {
        let params = make_pool_params();
        let encoded = encode_pool_params(&params);
        // Last byte should be 0xf6 (null) since pool_metadata is None
        assert_eq!(*encoded.last().unwrap(), 0xf6, "null pool_metadata");
    }

    #[test]
    fn test_encode_pool_params_with_metadata() {
        let mut params = make_pool_params();
        params.pool_metadata = Some(PoolMetadata {
            url: "https://my.pool".to_string(),
            hash: zero32(),
        });
        let encoded = encode_pool_params(&params);
        // Last byte should NOT be null — it should be part of hash32
        // The metadata section starts with array(2)
        // We just verify it doesn't end in 0xf6
        assert_ne!(*encoded.last().unwrap(), 0xf6, "non-null pool_metadata");
    }

    // ── encode_certificate ───────────────────────────────────────────────────

    /// StakeRegistration — array(2) tag=0
    #[test]
    fn test_encode_cert_stake_registration() {
        let cert = Certificate::StakeRegistration(key_cred());
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x82, "array(2)");
        assert_eq!(encoded[1], 0x00, "tag 0");
    }

    /// StakeDeregistration — array(2) tag=1
    #[test]
    fn test_encode_cert_stake_deregistration() {
        let cert = Certificate::StakeDeregistration(key_cred());
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x82, "array(2)");
        assert_eq!(encoded[1], 0x01, "tag 1");
    }

    /// StakeDelegation — array(3) tag=2
    #[test]
    fn test_encode_cert_stake_delegation() {
        let cert = Certificate::StakeDelegation {
            credential: key_cred(),
            pool_hash: zero28(),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x02, "tag 2");
    }

    /// PoolRegistration — array(10) tag=3
    #[test]
    fn test_encode_cert_pool_registration() {
        let cert = Certificate::PoolRegistration(make_pool_params());
        let encoded = encode_certificate(&cert);
        // array(10) = 0x8a
        assert_eq!(encoded[0], 0x8a, "array(10)");
        assert_eq!(encoded[1], 0x03, "tag 3");
    }

    /// PoolRetirement — array(3) tag=4
    #[test]
    fn test_encode_cert_pool_retirement() {
        let cert = Certificate::PoolRetirement {
            pool_hash: zero28(),
            epoch: 500,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x04, "tag 4");
        // epoch 500 = 0x19 0x01 0xf4
        assert_eq!(encoded[encoded.len() - 3], 0x19);
        let epoch = u16::from_be_bytes([encoded[encoded.len() - 2], encoded[encoded.len() - 1]]);
        assert_eq!(epoch, 500);
    }

    /// GenesisKeyDelegation — array(4) tag=5
    #[test]
    fn test_encode_cert_genesis_key_delegation() {
        let cert = Certificate::GenesisKeyDelegation {
            genesis_hash: zero32(),
            genesis_delegate_hash: zero32(),
            vrf_keyhash: zero32(),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84, "array(4)");
        assert_eq!(encoded[1], 0x05, "tag 5");
    }

    /// MoveInstantaneousRewards (OtherAccountingPot) — array(2) tag=6, inner array(2)
    #[test]
    fn test_encode_cert_move_instantaneous_rewards_other_pot() {
        let cert = Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::OtherAccountingPot(1_000_000),
        };
        let encoded = encode_certificate(&cert);
        // Outer array(2)
        assert_eq!(encoded[0], 0x82, "outer array(2)");
        assert_eq!(encoded[1], 0x06, "tag 6");
        // MIR body: array(2) [source=0, coin]
        assert_eq!(encoded[2], 0x82, "inner array(2)");
        assert_eq!(encoded[3], 0x00, "source=Reserves(0)");
        // 1_000_000 = 0x1a 0x00 0x0f 0x42 0x40
        assert_eq!(encoded[4], 0x1a);
        let coin = u32::from_be_bytes([encoded[5], encoded[6], encoded[7], encoded[8]]);
        assert_eq!(coin, 1_000_000);
    }

    #[test]
    fn test_encode_cert_mir_treasury_source() {
        let cert = Certificate::MoveInstantaneousRewards {
            source: MIRSource::Treasury,
            target: MIRTarget::OtherAccountingPot(0),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[3], 0x01, "source=Treasury(1)");
    }

    /// ConwayStakeRegistration — array(3) tag=7
    #[test]
    fn test_encode_cert_conway_stake_registration() {
        let cert = Certificate::ConwayStakeRegistration {
            credential: key_cred(),
            deposit: Lovelace(2_000_000),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x07, "tag 7");
    }

    /// ConwayStakeDeregistration — array(3) tag=8
    #[test]
    fn test_encode_cert_conway_stake_deregistration() {
        let cert = Certificate::ConwayStakeDeregistration {
            credential: key_cred(),
            refund: Lovelace(2_000_000),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x08, "tag 8");
    }

    /// VoteDelegation — array(3) tag=9
    #[test]
    fn test_encode_cert_vote_delegation() {
        let cert = Certificate::VoteDelegation {
            credential: key_cred(),
            drep: DRep::Abstain,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x09, "tag 9");
    }

    /// StakeVoteDelegation — array(4) tag=10 (0x0a)
    #[test]
    fn test_encode_cert_stake_vote_delegation() {
        let cert = Certificate::StakeVoteDelegation {
            credential: key_cred(),
            pool_hash: zero28(),
            drep: DRep::NoConfidence,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84, "array(4)");
        assert_eq!(encoded[1], 0x0a, "tag 10");
    }

    /// RegStakeDeleg — array(4) tag=11 (0x0b)
    #[test]
    fn test_encode_cert_reg_stake_deleg() {
        let cert = Certificate::RegStakeDeleg {
            credential: key_cred(),
            pool_hash: zero28(),
            deposit: Lovelace(2_000_000),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84, "array(4)");
        assert_eq!(encoded[1], 0x0b, "tag 11");
    }

    /// VoteRegDeleg — array(4) tag=12 (0x0c)
    #[test]
    fn test_encode_cert_vote_reg_deleg() {
        let cert = Certificate::VoteRegDeleg {
            credential: key_cred(),
            drep: DRep::KeyHash(zero32()),
            deposit: Lovelace(2_000_000),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84, "array(4)");
        assert_eq!(encoded[1], 0x0c, "tag 12");
    }

    /// RegStakeVoteDeleg — array(5) tag=13 (0x0d)
    #[test]
    fn test_encode_cert_reg_stake_vote_deleg() {
        let cert = Certificate::RegStakeVoteDeleg {
            credential: key_cred(),
            pool_hash: zero28(),
            drep: DRep::Abstain,
            deposit: Lovelace(2_000_000),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x85, "array(5)");
        assert_eq!(encoded[1], 0x0d, "tag 13");
    }

    /// CommitteeHotAuth — array(3) tag=14 (0x0e)
    #[test]
    fn test_encode_cert_committee_hot_auth() {
        let cert = Certificate::CommitteeHotAuth {
            cold_credential: key_cred(),
            hot_credential: script_cred(),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x0e, "tag 14");
    }

    /// CommitteeColdResign — array(3) tag=15 (0x0f)
    #[test]
    fn test_encode_cert_committee_cold_resign_no_anchor() {
        let cert = Certificate::CommitteeColdResign {
            cold_credential: key_cred(),
            anchor: None,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x0f, "tag 15");
        // anchor = null = 0xf6 (last byte)
        assert_eq!(*encoded.last().unwrap(), 0xf6, "null anchor");
    }

    #[test]
    fn test_encode_cert_committee_cold_resign_with_anchor() {
        let cert = Certificate::CommitteeColdResign {
            cold_credential: key_cred(),
            anchor: Some(Anchor {
                url: "https://example.com".to_string(),
                data_hash: zero32(),
            }),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x0f, "tag 15");
        // anchor is present — last byte is NOT 0xf6
        assert_ne!(*encoded.last().unwrap(), 0xf6, "anchor present, not null");
    }

    /// RegDRep — array(4) tag=16 (0x10)
    #[test]
    fn test_encode_cert_reg_drep() {
        let cert = Certificate::RegDRep {
            credential: key_cred(),
            deposit: Lovelace(500_000_000),
            anchor: None,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x84, "array(4)");
        assert_eq!(encoded[1], 0x10, "tag 16");
        assert_eq!(*encoded.last().unwrap(), 0xf6, "null anchor");
    }

    /// UnregDRep — array(3) tag=17 (0x11)
    #[test]
    fn test_encode_cert_unreg_drep() {
        let cert = Certificate::UnregDRep {
            credential: key_cred(),
            refund: Lovelace(500_000_000),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x11, "tag 17");
    }

    /// UpdateDRep — array(3) tag=18 (0x12)
    #[test]
    fn test_encode_cert_update_drep_no_anchor() {
        let cert = Certificate::UpdateDRep {
            credential: key_cred(),
            anchor: None,
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x12, "tag 18");
        assert_eq!(*encoded.last().unwrap(), 0xf6, "null anchor");
    }

    #[test]
    fn test_encode_cert_update_drep_with_anchor() {
        let cert = Certificate::UpdateDRep {
            credential: key_cred(),
            anchor: Some(Anchor {
                url: "https://drep.example.com".to_string(),
                data_hash: zero32(),
            }),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x12, "tag 18");
        assert_ne!(*encoded.last().unwrap(), 0xf6, "anchor present");
    }

    // ── MIR StakeCredentials variant ─────────────────────────────────────────

    #[test]
    fn test_encode_cert_mir_stake_credentials() {
        let cert = Certificate::MoveInstantaneousRewards {
            source: MIRSource::Reserves,
            target: MIRTarget::StakeCredentials(vec![(key_cred(), 1_000_000)]),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x82, "outer array(2)");
        assert_eq!(encoded[1], 0x06, "tag 6");
        assert_eq!(encoded[2], 0x82, "inner array(2)");
        assert_eq!(encoded[3], 0x00, "source=Reserves");
        // map(1) = 0xa1
        assert_eq!(encoded[4], 0xa1, "map(1) for 1 credential");
    }

    // ── DRep variants inside VoteDelegation ──────────────────────────────────

    #[test]
    fn test_encode_cert_vote_delegation_script_hash_drep() {
        let cert = Certificate::VoteDelegation {
            credential: key_cred(),
            drep: DRep::ScriptHash(Hash28::from_bytes([0xab; 28])),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83, "array(3)");
        assert_eq!(encoded[1], 0x09, "tag 9");
        // drep bytes follow the credential (30 bytes from position 2)
        // credential is array(2) [0x00, bstr(28)] = 32 bytes total (0x82 + 0x00 + 0x58 + 0x1c + 28)
        // just ensure encoded is non-empty and tag is correct
        assert!(encoded.len() > 5);
    }

    #[test]
    fn test_encode_cert_vote_delegation_keyhash_drep() {
        let cert = Certificate::VoteDelegation {
            credential: key_cred(),
            drep: DRep::KeyHash(zero32()),
        };
        let encoded = encode_certificate(&cert);
        assert_eq!(encoded[0], 0x83);
        assert_eq!(encoded[1], 0x09);
    }
}
