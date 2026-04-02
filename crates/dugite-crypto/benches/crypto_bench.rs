//! Criterion benchmarks for cryptographic operations.
//!
//! Scales based on Cardano mainnet reference numbers:
//! - Witnesses per block: 10-500 (average ~50)
//! - Blocks can have up to 500+ witnesses for large multi-sig or DApp blocks
//!
//! Run:  cargo bench -p dugite-crypto
//! HTML: target/criterion/report/index.html

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use dugite_crypto::keys::PaymentSigningKey;
use dugite_crypto::vrf::verify_vrf_proof;
use dugite_primitives::hash::blake2b_224;

fn bench_ed25519_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("ed25519_verify");

    // Generate a key pair and sign a message
    let sk = PaymentSigningKey::generate();
    let vk = sk.verification_key();
    let message = [0xABu8; 32]; // typical tx hash
    let signature = sk.sign(&message);

    group.bench_function("single", |b| {
        b.iter(|| vk.verify(black_box(&message), black_box(&signature)))
    });

    group.finish();
}

fn bench_ed25519_batch_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("ed25519_batch_verify");

    // Mainnet blocks can have 1-500+ witnesses
    for count in [1, 10, 50, 100, 200, 500] {
        let witnesses: Vec<_> = (0..count)
            .map(|_| {
                let sk = PaymentSigningKey::generate();
                let vk = sk.verification_key();
                let message = [0xABu8; 32];
                let signature = sk.sign(&message);
                (vk, message, signature)
            })
            .collect();

        group.bench_with_input(
            BenchmarkId::new("sequential", count),
            &witnesses,
            |b, witnesses| {
                b.iter(|| {
                    for (vk, msg, sig) in witnesses {
                        vk.verify(msg, sig).unwrap();
                        black_box(());
                    }
                })
            },
        );
    }

    group.finish();
}

fn bench_keyhash_from_vkey(c: &mut Criterion) {
    let mut group = c.benchmark_group("keyhash_from_vkey");

    // Benchmark blake2b_224(vkey) -- done for every witness during validation
    let sk = PaymentSigningKey::generate();
    let vk_bytes = sk.verification_key().to_bytes();

    group.bench_function("single", |b| b.iter(|| blake2b_224(black_box(&vk_bytes))));

    // Mainnet batch sizes: 10-500 keys per block
    for count in [10, 50, 100, 200, 500] {
        let keys: Vec<[u8; 32]> = (0..count)
            .map(|_| PaymentSigningKey::generate().verification_key().to_bytes())
            .collect();

        group.bench_with_input(BenchmarkId::new("batch", count), &keys, |b, keys| {
            b.iter(|| {
                for key in keys {
                    black_box(blake2b_224(key));
                }
            })
        });
    }

    group.finish();
}

fn bench_vrf_verify(c: &mut Criterion) {
    let mut group = c.benchmark_group("vrf_verify");

    // Generate a VRF key pair and proof
    use vrf_dalek::vrf03::{PublicKey03 as VrfPublicKey, SecretKey03 as VrfSecretKey, VrfProof03};

    let vrf_seed = [42u8; 32];
    let vrf_sk = VrfSecretKey::from_bytes(&vrf_seed);
    let vrf_pk = VrfPublicKey::from(&vrf_sk);
    let vrf_pk_bytes = vrf_pk.as_bytes().to_vec();
    let alpha = [0xABu8; 32]; // nonce seed
    let proof = VrfProof03::generate(&vrf_pk, &vrf_sk, &alpha);
    let proof_bytes = proof.to_bytes().to_vec();

    group.bench_function("single_proof", |b| {
        b.iter(|| {
            let result = verify_vrf_proof(
                black_box(&vrf_pk_bytes),
                black_box(&proof_bytes),
                black_box(&alpha),
            );
            black_box(result.is_ok());
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_ed25519_verify,
    bench_ed25519_batch_verify,
    bench_keyhash_from_vkey,
    bench_vrf_verify
);
criterion_main!(benches);
