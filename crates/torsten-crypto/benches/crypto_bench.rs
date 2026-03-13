use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use torsten_crypto::keys::PaymentSigningKey;
use torsten_primitives::hash::blake2b_224;

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

    // Simulate batch witness verification (sequential, matching block validation)
    for count in [1, 5, 10, 25, 50] {
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

    // Benchmark blake2b_224(vkey) — done for every witness during validation
    let sk = PaymentSigningKey::generate();
    let vk_bytes = sk.verification_key().to_bytes();

    group.bench_function("single", |b| b.iter(|| blake2b_224(black_box(&vk_bytes))));

    // Batch: typical block witness counts
    for count in [5, 10, 50, 100] {
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

criterion_group!(
    benches,
    bench_ed25519_verify,
    bench_ed25519_batch_verify,
    bench_keyhash_from_vkey
);
criterion_main!(benches);
