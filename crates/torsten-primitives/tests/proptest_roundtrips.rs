//! Property-based tests for primitive type round-trips.
//!
//! Uses `proptest` to verify that serialization round-trips (to_bytes/from_bytes,
//! to_hex/from_hex) are identity functions for all core Cardano types.

use proptest::prelude::*;
use torsten_primitives::address::*;
use torsten_primitives::credentials::{Credential, Pointer};
use torsten_primitives::hash::{Hash28, Hash32};
use torsten_primitives::network::NetworkId;
use torsten_primitives::value::{AssetName, Lovelace, Value};

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn arb_hash32() -> impl Strategy<Value = Hash32> {
    prop::array::uniform32(any::<u8>()).prop_map(Hash32::from_bytes)
}

fn arb_hash28() -> impl Strategy<Value = Hash28> {
    prop::array::uniform28(any::<u8>()).prop_map(Hash28::from_bytes)
}

fn arb_network_id() -> impl Strategy<Value = NetworkId> {
    prop_oneof![Just(NetworkId::Testnet), Just(NetworkId::Mainnet),]
}

fn arb_credential() -> impl Strategy<Value = Credential> {
    prop_oneof![
        arb_hash28().prop_map(Credential::VerificationKey),
        arb_hash28().prop_map(Credential::Script),
    ]
}

fn arb_base_address() -> impl Strategy<Value = Address> {
    (arb_network_id(), arb_credential(), arb_credential()).prop_map(|(network, payment, stake)| {
        Address::Base(BaseAddress {
            network,
            payment,
            stake,
        })
    })
}

fn arb_enterprise_address() -> impl Strategy<Value = Address> {
    (arb_network_id(), arb_credential())
        .prop_map(|(network, payment)| Address::Enterprise(EnterpriseAddress { network, payment }))
}

fn arb_reward_address() -> impl Strategy<Value = Address> {
    (arb_network_id(), arb_credential())
        .prop_map(|(network, stake)| Address::Reward(RewardAddress { network, stake }))
}

/// Pointer address strategy with bounded pointer values (variable-length
/// encoding must fit within reasonable bounds).
fn arb_pointer_address() -> impl Strategy<Value = Address> {
    (
        arb_network_id(),
        arb_credential(),
        0u64..=1_000_000_000,
        0u64..=10_000,
        0u64..=1_000,
    )
        .prop_map(|(network, payment, slot, tx_index, cert_index)| {
            Address::Pointer(PointerAddress {
                network,
                payment,
                pointer: Pointer {
                    slot,
                    tx_index,
                    cert_index,
                },
            })
        })
}

fn arb_shelley_address() -> impl Strategy<Value = Address> {
    prop_oneof![
        arb_base_address(),
        arb_enterprise_address(),
        arb_reward_address(),
        arb_pointer_address(),
    ]
}

fn arb_asset_name() -> impl Strategy<Value = AssetName> {
    prop::collection::vec(any::<u8>(), 0..=32).prop_map(AssetName)
}

// ===========================================================================
// Property-based tests
// ===========================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    // -----------------------------------------------------------------------
    // Hash32: hex round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_hex_roundtrip(hash in arb_hash32()) {
        let hex_str = hash.to_hex();
        let recovered = Hash32::from_hex(&hex_str).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    // -----------------------------------------------------------------------
    // Hash28: hex round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash28_hex_roundtrip(hash in arb_hash28()) {
        let hex_str = hash.to_hex();
        let recovered = Hash28::from_hex(&hex_str).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    // -----------------------------------------------------------------------
    // Hash: from_bytes / as_bytes identity
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_bytes_roundtrip(bytes in prop::array::uniform32(any::<u8>())) {
        let hash = Hash32::from_bytes(bytes);
        prop_assert_eq!(*hash.as_bytes(), bytes);
    }

    #[test]
    fn prop_hash28_bytes_roundtrip(bytes in prop::array::uniform28(any::<u8>())) {
        let hash = Hash28::from_bytes(bytes);
        prop_assert_eq!(*hash.as_bytes(), bytes);
    }

    // -----------------------------------------------------------------------
    // Hash: TryFrom<&[u8]> round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_try_from_slice(bytes in prop::array::uniform32(any::<u8>())) {
        let hash = Hash32::from_bytes(bytes);
        let slice: &[u8] = hash.as_ref();
        let recovered = Hash32::try_from(slice).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    #[test]
    fn prop_hash28_try_from_slice(bytes in prop::array::uniform28(any::<u8>())) {
        let hash = Hash28::from_bytes(bytes);
        let slice: &[u8] = hash.as_ref();
        let recovered = Hash28::try_from(slice).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    // -----------------------------------------------------------------------
    // Hash28 -> Hash32 padding: first 28 bytes preserved, last 4 are zero
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash28_to_hash32_padding(hash in arb_hash28()) {
        let h32 = hash.to_hash32_padded();
        prop_assert_eq!(&h32.as_bytes()[..28], hash.as_bytes());
        prop_assert_eq!(&h32.as_bytes()[28..], &[0u8; 4]);
    }

    // -----------------------------------------------------------------------
    // Hash: Display and from_hex round-trip (Display uses hex)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_display_roundtrip(hash in arb_hash32()) {
        let display_str = format!("{}", hash);
        let recovered = Hash32::from_hex(&display_str).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    // -----------------------------------------------------------------------
    // Address: to_bytes / from_bytes round-trip (Shelley address types)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_address_bytes_roundtrip(addr in arb_shelley_address()) {
        let bytes = addr.to_bytes();
        let decoded = Address::from_bytes(&bytes).unwrap();
        prop_assert_eq!(addr, decoded);
    }

    // -----------------------------------------------------------------------
    // Address: Base address is always 57 bytes
    // -----------------------------------------------------------------------
    #[test]
    fn prop_base_address_length(addr in arb_base_address()) {
        let bytes = addr.to_bytes();
        prop_assert_eq!(bytes.len(), 57);
    }

    // -----------------------------------------------------------------------
    // Address: Enterprise address is always 29 bytes
    // -----------------------------------------------------------------------
    #[test]
    fn prop_enterprise_address_length(addr in arb_enterprise_address()) {
        let bytes = addr.to_bytes();
        prop_assert_eq!(bytes.len(), 29);
    }

    // -----------------------------------------------------------------------
    // Address: Reward address is always 29 bytes
    // -----------------------------------------------------------------------
    #[test]
    fn prop_reward_address_length(addr in arb_reward_address()) {
        let bytes = addr.to_bytes();
        prop_assert_eq!(bytes.len(), 29);
    }

    // -----------------------------------------------------------------------
    // Address: network_id is preserved
    // -----------------------------------------------------------------------
    #[test]
    fn prop_address_preserves_network(addr in arb_shelley_address()) {
        let expected_net = addr.network_id();
        let bytes = addr.to_bytes();
        let decoded = Address::from_bytes(&bytes).unwrap();
        prop_assert_eq!(decoded.network_id(), expected_net);
    }

    // -----------------------------------------------------------------------
    // Address: payment_credential is preserved
    // -----------------------------------------------------------------------
    #[test]
    fn prop_address_preserves_payment_cred(addr in arb_shelley_address()) {
        let expected_cred = addr.payment_credential().cloned();
        let bytes = addr.to_bytes();
        let decoded = Address::from_bytes(&bytes).unwrap();
        prop_assert_eq!(decoded.payment_credential().cloned(), expected_cred);
    }

    // -----------------------------------------------------------------------
    // NetworkId: to_u8 / from_u8 round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_network_id_roundtrip(net in arb_network_id()) {
        let byte = net.to_u8();
        let recovered = NetworkId::from_u8(byte).unwrap();
        prop_assert_eq!(net, recovered);
    }

    // -----------------------------------------------------------------------
    // Lovelace: checked_add commutativity
    // -----------------------------------------------------------------------
    #[test]
    fn prop_lovelace_add_commutative(a in 0u64..=u64::MAX / 2, b in 0u64..=u64::MAX / 2) {
        let la = Lovelace(a);
        let lb = Lovelace(b);
        prop_assert_eq!(la.checked_add(lb), lb.checked_add(la));
    }

    // -----------------------------------------------------------------------
    // Lovelace: checked_add then checked_sub identity
    // -----------------------------------------------------------------------
    #[test]
    fn prop_lovelace_add_sub_identity(a in 0u64..=u64::MAX / 2, b in 0u64..=u64::MAX / 2) {
        let la = Lovelace(a);
        let lb = Lovelace(b);
        let sum = la.checked_add(lb).unwrap();
        let result = sum.checked_sub(lb).unwrap();
        prop_assert_eq!(result, la);
    }

    // -----------------------------------------------------------------------
    // Value: ADA-only geq reflexivity
    // -----------------------------------------------------------------------
    #[test]
    fn prop_value_geq_reflexive(coin in any::<u64>()) {
        let v = Value::lovelace(coin);
        prop_assert!(v.geq(&v));
    }

    // -----------------------------------------------------------------------
    // Value: add identity (adding zero)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_value_add_zero_identity(coin in any::<u64>()) {
        let v = Value::lovelace(coin);
        let zero = Value::lovelace(0);
        let result = v.add(&zero);
        prop_assert_eq!(result.coin, v.coin);
        prop_assert!(result.multi_asset.is_empty());
    }

    // -----------------------------------------------------------------------
    // Value: is_pure_ada for ADA-only values
    // -----------------------------------------------------------------------
    #[test]
    fn prop_value_ada_only_is_pure(coin in any::<u64>()) {
        let v = Value::lovelace(coin);
        prop_assert!(v.is_pure_ada());
    }

    // -----------------------------------------------------------------------
    // AssetName: length constraint (0..=32 bytes)
    // -----------------------------------------------------------------------
    #[test]
    fn prop_asset_name_valid_length(name in arb_asset_name()) {
        prop_assert!(name.0.len() <= 32);
    }

    // -----------------------------------------------------------------------
    // AssetName: new() rejects names > 32 bytes
    // -----------------------------------------------------------------------
    #[test]
    fn prop_asset_name_rejects_too_long(len in 33usize..=128) {
        let bytes = vec![0u8; len];
        prop_assert!(AssetName::new(bytes).is_err());
    }

    // -----------------------------------------------------------------------
    // Hash: serde JSON round-trip
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_serde_roundtrip(hash in arb_hash32()) {
        let json = serde_json::to_string(&hash).unwrap();
        let recovered: Hash32 = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    #[test]
    fn prop_hash28_serde_roundtrip(hash in arb_hash28()) {
        let json = serde_json::to_string(&hash).unwrap();
        let recovered: Hash28 = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(hash, recovered);
    }

    // -----------------------------------------------------------------------
    // Lovelace: Display always ends with " lovelace"
    // -----------------------------------------------------------------------
    #[test]
    fn prop_lovelace_display(amount in any::<u64>()) {
        let l = Lovelace(amount);
        let display = format!("{}", l);
        prop_assert!(display.ends_with(" lovelace"));
        prop_assert!(display.starts_with(&amount.to_string()));
    }

    // -----------------------------------------------------------------------
    // Hash: ordering is consistent with byte ordering
    // -----------------------------------------------------------------------
    #[test]
    fn prop_hash32_ordering_consistent(a in arb_hash32(), b in arb_hash32()) {
        let ord_hash = a.cmp(&b);
        let ord_bytes = a.as_bytes().cmp(b.as_bytes());
        prop_assert_eq!(ord_hash, ord_bytes);
    }

    #[test]
    fn prop_hash28_ordering_consistent(a in arb_hash28(), b in arb_hash28()) {
        let ord_hash = a.cmp(&b);
        let ord_bytes = a.as_bytes().cmp(b.as_bytes());
        prop_assert_eq!(ord_hash, ord_bytes);
    }

    // -----------------------------------------------------------------------
    // Pointer address: variable-length encoding round-trip through address
    // -----------------------------------------------------------------------
    #[test]
    fn prop_pointer_address_roundtrip(
        network in arb_network_id(),
        cred in arb_credential(),
        slot in 0u64..=100_000_000,
        tx_index in 0u64..=10_000,
        cert_index in 0u64..=1_000,
    ) {
        let addr = Address::Pointer(PointerAddress {
            network,
            payment: cred,
            pointer: Pointer { slot, tx_index, cert_index },
        });
        let bytes = addr.to_bytes();
        let decoded = Address::from_bytes(&bytes).unwrap();
        prop_assert_eq!(addr, decoded);
    }
}
