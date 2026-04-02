use crate::helpers::cli::run_cli_ok;
use tempfile::TempDir;

/// Ephemeral payment key pair in a temporary directory.
pub struct TempKeys {
    pub dir: TempDir,
    pub payment_skey: String,
    pub payment_vkey: String,
}

impl TempKeys {
    /// Generate a new ephemeral payment key pair.
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let skey = dir.path().join("payment.skey").display().to_string();
        let vkey = dir.path().join("payment.vkey").display().to_string();

        run_cli_ok(&[
            "address",
            "key-gen",
            "--signing-key-file",
            &skey,
            "--verification-key-file",
            &vkey,
        ]);

        Self {
            dir,
            payment_skey: skey,
            payment_vkey: vkey,
        }
    }

    /// Build a testnet enterprise address from this key pair.
    pub fn enterprise_address_testnet(&self) -> String {
        run_cli_ok(&[
            "address",
            "build",
            "--payment-verification-key-file",
            &self.payment_vkey,
            "--network",
            "testnet",
        ])
        .trim()
        .to_string()
    }

    /// Build a mainnet enterprise address from this key pair.
    pub fn enterprise_address_mainnet(&self) -> String {
        run_cli_ok(&[
            "address",
            "build",
            "--payment-verification-key-file",
            &self.payment_vkey,
            "--network",
            "mainnet",
        ])
        .trim()
        .to_string()
    }

    /// Get the payment key hash.
    pub fn payment_key_hash(&self) -> String {
        run_cli_ok(&[
            "address",
            "key-hash",
            "--payment-verification-key-file",
            &self.payment_vkey,
        ])
        .trim()
        .to_string()
    }
}

/// Ephemeral stake key pair in a temporary directory.
pub struct TempStakeKeys {
    pub dir: TempDir,
    pub stake_skey: String,
    pub stake_vkey: String,
}

impl TempStakeKeys {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let skey = dir.path().join("stake.skey").display().to_string();
        let vkey = dir.path().join("stake.vkey").display().to_string();

        run_cli_ok(&[
            "stake-address",
            "key-gen",
            "--signing-key-file",
            &skey,
            "--verification-key-file",
            &vkey,
        ]);

        Self {
            dir,
            stake_skey: skey,
            stake_vkey: vkey,
        }
    }
}

/// Ephemeral node cold keys + opcert counter.
pub struct TempNodeKeys {
    pub dir: TempDir,
    pub cold_skey: String,
    pub cold_vkey: String,
    pub counter_file: String,
}

impl TempNodeKeys {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let cold_skey = dir.path().join("cold.skey").display().to_string();
        let cold_vkey = dir.path().join("cold.vkey").display().to_string();
        let counter = dir.path().join("opcert.counter").display().to_string();

        run_cli_ok(&[
            "node",
            "key-gen",
            "--cold-signing-key-file",
            &cold_skey,
            "--cold-verification-key-file",
            &cold_vkey,
            "--operational-certificate-counter-file",
            &counter,
        ]);

        Self {
            dir,
            cold_skey,
            cold_vkey,
            counter_file: counter,
        }
    }
}

/// Ephemeral KES key pair.
pub struct TempKesKeys {
    pub dir: TempDir,
    pub kes_skey: String,
    pub kes_vkey: String,
}

impl TempKesKeys {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let skey = dir.path().join("kes.skey").display().to_string();
        let vkey = dir.path().join("kes.vkey").display().to_string();

        run_cli_ok(&[
            "node",
            "key-gen-kes",
            "--signing-key-file",
            &skey,
            "--verification-key-file",
            &vkey,
        ]);

        Self {
            dir,
            kes_skey: skey,
            kes_vkey: vkey,
        }
    }
}

/// Ephemeral VRF key pair.
pub struct TempVrfKeys {
    pub dir: TempDir,
    pub vrf_skey: String,
    pub vrf_vkey: String,
}

impl TempVrfKeys {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let skey = dir.path().join("vrf.skey").display().to_string();
        let vkey = dir.path().join("vrf.vkey").display().to_string();

        run_cli_ok(&[
            "node",
            "key-gen-vrf",
            "--signing-key-file",
            &skey,
            "--verification-key-file",
            &vkey,
        ]);

        Self {
            dir,
            vrf_skey: skey,
            vrf_vkey: vkey,
        }
    }
}

/// Ephemeral DRep key pair.
pub struct TempDrepKeys {
    pub dir: TempDir,
    pub drep_skey: String,
    pub drep_vkey: String,
}

impl TempDrepKeys {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("Failed to create temp dir");
        let skey = dir.path().join("drep.skey").display().to_string();
        let vkey = dir.path().join("drep.vkey").display().to_string();

        run_cli_ok(&[
            "governance",
            "drep",
            "key-gen",
            "--signing-key-file",
            &skey,
            "--verification-key-file",
            &vkey,
        ]);

        Self {
            dir,
            drep_skey: skey,
            drep_vkey: vkey,
        }
    }
}
