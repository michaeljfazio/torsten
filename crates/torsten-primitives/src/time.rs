use serde::{Deserialize, Serialize};

/// Absolute slot number (monotonically increasing across all eras)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SlotNo(pub u64);

/// Epoch number
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EpochNo(pub u64);

/// Block number (height)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BlockNo(pub u64);

/// POSIX time in milliseconds (used in Plutus scripts)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PosixTimeMillis(pub i64);

/// Shelley-era time parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStart {
    /// UTC time when the blockchain started (Byron genesis)
    pub utc_time: chrono::DateTime<chrono::Utc>,
}

/// Slot length in seconds
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SlotLength(pub f64);

/// Epoch length in slots
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct EpochLength(pub u64);

impl SlotNo {
    pub fn to_epoch(self, epoch_length: EpochLength) -> EpochNo {
        EpochNo(self.0 / epoch_length.0)
    }

    pub fn slot_in_epoch(self, epoch_length: EpochLength) -> u64 {
        self.0 % epoch_length.0
    }

    pub fn to_posix_time(
        self,
        system_start: &SystemStart,
        slot_length: SlotLength,
    ) -> PosixTimeMillis {
        let elapsed_ms = (self.0 as f64 * slot_length.0 * 1000.0) as i64;
        PosixTimeMillis(system_start.utc_time.timestamp_millis() + elapsed_ms)
    }
}

impl BlockNo {
    pub fn next(self) -> Self {
        BlockNo(self.0 + 1)
    }
}

impl std::fmt::Display for SlotNo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "slot:{}", self.0)
    }
}

impl std::fmt::Display for EpochNo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "epoch:{}", self.0)
    }
}

impl std::fmt::Display for BlockNo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "block:{}", self.0)
    }
}

/// Mainnet system start (September 23, 2017)
pub fn mainnet_system_start() -> SystemStart {
    use chrono::TimeZone;
    SystemStart {
        utc_time: chrono::Utc
            .with_ymd_and_hms(2017, 9, 23, 21, 44, 51)
            .unwrap(),
    }
}

/// Mainnet Shelley epoch length (432000 slots = 5 days)
pub fn mainnet_epoch_length() -> EpochLength {
    EpochLength(432000)
}

/// Mainnet slot length (1 second since Shelley)
pub fn mainnet_slot_length() -> SlotLength {
    SlotLength(1.0)
}
