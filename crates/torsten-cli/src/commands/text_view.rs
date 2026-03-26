use anyhow::Result;
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct TextViewCmd {
    #[command(subcommand)]
    command: TextViewSubcommand,
}

#[derive(Subcommand, Debug)]
enum TextViewSubcommand {
    /// Decode a text-view file to its CBOR representation
    DecodeCbor {
        /// Input text-view file
        #[arg(long)]
        file: PathBuf,
    },
}

impl TextViewCmd {
    pub fn run(self) -> Result<()> {
        match self.command {
            TextViewSubcommand::DecodeCbor { file } => {
                let content = std::fs::read_to_string(&file)?;
                let env: serde_json::Value = serde_json::from_str(&content)?;

                let type_str = env["type"].as_str().unwrap_or("Unknown");
                let description = env["description"].as_str().unwrap_or("");
                let cbor_hex = env["cborHex"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing cborHex in {}", file.display()))?;
                let cbor_bytes = hex::decode(cbor_hex)?;

                println!("Type: {}", type_str);
                if !description.is_empty() {
                    println!("Description: {}", description);
                }
                println!(
                    "Hash: {}",
                    hex::encode(torsten_primitives::hash::blake2b_256(&cbor_bytes))
                );
                println!();
                println!("CBOR decoded:");
                decode_cbor_display(&cbor_bytes, &mut 0);
                println!();
                println!("Raw CBOR bytes: {}", cbor_hex);
                println!("CBOR byte length: {} bytes", cbor_bytes.len());

                Ok(())
            }
        }
    }
}

fn decode_cbor_display(data: &[u8], indent: &mut usize) {
    let spaces = "  ".repeat(*indent);
    let mut offset = 0;

    while offset < data.len() {
        let (item_len, item_display) = decode_cbor_item(&data[offset..], *indent + 1);
        println!("{}{}", spaces, item_display);
        offset += item_len;
        if item_len == 0 {
            break;
        }
    }
}

fn decode_cbor_item(data: &[u8], indent: usize) -> (usize, String) {
    if data.is_empty() {
        return (0, "<empty>".to_string());
    }

    let spaces = "  ".repeat(indent);
    let first = data[0];

    match first & 0xe0 {
        0x00..=0x17 => (1, format!("Unsigned integer: {}", first & 0x1f)),
        0x18 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            (2, format!("Unsigned integer: {}", data[1]))
        }
        0x19 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let val = u16::from_be_bytes([data[1], data[2]]);
            (3, format!("Unsigned integer: {}", val))
        }
        0x1a => {
            if data.len() < 5 {
                return (0, "<incomplete>".to_string());
            }
            let val = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            (5, format!("Unsigned integer: {}", val))
        }
        0x1b => {
            if data.len() < 9 {
                return (0, "<incomplete>".to_string());
            }
            let val = u64::from_be_bytes([
                data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
            ]);
            (9, format!("Unsigned integer: {}", val))
        }
        0x20..=0x37 => {
            let val = (first & 0x1f) as i8;
            (1, format!("Negative integer: {}", val as i64 - 1))
        }
        0x38 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            (2, format!("Negative integer: {}", -(data[1] as i64) - 1))
        }
        0x39 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let val = u16::from_be_bytes([data[1], data[2]]);
            (3, format!("Negative integer: {}", -(val as i64) - 1))
        }
        0x3a => {
            if data.len() < 5 {
                return (0, "<incomplete>".to_string());
            }
            let val = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            (5, format!("Negative integer: {}", -(val as i64) - 1))
        }
        0x3b => {
            if data.len() < 9 {
                return (0, "<incomplete>".to_string());
            }
            let val = u64::from_be_bytes([
                data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
            ]);
            (9, format!("Negative integer: {}", -(val as i64) - 1))
        }
        0x40..=0x57 => {
            let len = (first & 0x37) as usize;
            if data.len() < 1 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[1..1 + len];
            (1 + len, format!("Bytes ({}): {}", len, hex::encode(bytes)))
        }
        0x58 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            let len = data[1] as usize;
            if data.len() < 2 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[2..2 + len];
            (2 + len, format!("Bytes ({}): {}", len, hex::encode(bytes)))
        }
        0x59 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let len = u16::from_be_bytes([data[1], data[2]]) as usize;
            if data.len() < 3 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[3..3 + len];
            (3 + len, format!("Bytes ({}): {}", len, hex::encode(bytes)))
        }
        0x5a => {
            if data.len() < 5 {
                return (0, "<incomplete>".to_string());
            }
            let len = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
            if data.len() < 5 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[5..5 + len];
            (5 + len, format!("Bytes ({}): {}", len, hex::encode(bytes)))
        }
        0x5b => {
            if data.len() < 9 {
                return (0, "<incomplete>".to_string());
            }
            let len = u64::from_be_bytes([
                data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
            ]) as usize;
            if data.len() < 9 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[9..9 + len];
            (9 + len, format!("Bytes ({}): {}", len, hex::encode(bytes)))
        }
        0x60..=0x77 => {
            let len = (first & 0x1f) as usize;
            if data.len() < 1 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[1..1 + len];
            match String::from_utf8(bytes.to_vec()) {
                Ok(s) => (1 + len, format!("Text: \"{}\"", s)),
                Err(_) => (
                    1 + len,
                    format!("Text (invalid UTF-8): {}", hex::encode(bytes)),
                ),
            }
        }
        0x78 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            let len = data[1] as usize;
            if data.len() < 2 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[2..2 + len];
            match String::from_utf8(bytes.to_vec()) {
                Ok(s) => (2 + len, format!("Text ({}): \"{}\"", len, s)),
                Err(_) => (
                    2 + len,
                    format!("Text (invalid UTF-8): {}", hex::encode(bytes)),
                ),
            }
        }
        0x79 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let len = u16::from_be_bytes([data[1], data[2]]) as usize;
            if data.len() < 3 + len {
                return (0, "<incomplete>".to_string());
            }
            let bytes = &data[3..3 + len];
            match String::from_utf8(bytes.to_vec()) {
                Ok(s) => (3 + len, format!("Text ({}): \"{}\"", len, s)),
                Err(_) => (
                    3 + len,
                    format!("Text (invalid UTF-8): {}", hex::encode(bytes)),
                ),
            }
        }
        0x7a..=0x7b => (1, "Text: <large text not decoded>".to_string()),
        0x80..=0x97 => {
            let len = (first & 0x1f) as usize;
            let mut offset = 1;
            let mut items = Vec::new();
            for _ in 0..len {
                if offset >= data.len() {
                    break;
                }
                let (item_len, item_str) = decode_cbor_item(&data[offset..], indent + 1);
                if item_len == 0 {
                    break;
                }
                items.push(format!("{}{}", spaces, item_str));
                offset += item_len;
            }
            (offset, format!("Array ({}):\n{}", len, items.join("\n")))
        }
        0x98 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            let len = data[1] as usize;
            let mut offset = 2;
            let mut items = Vec::new();
            for _ in 0..len {
                if offset >= data.len() {
                    break;
                }
                let (item_len, item_str) = decode_cbor_item(&data[offset..], indent + 1);
                if item_len == 0 {
                    break;
                }
                items.push(format!("{}{}", spaces, item_str));
                offset += item_len;
            }
            (offset, format!("Array ({}):\n{}", len, items.join("\n")))
        }
        0x99 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let len = u16::from_be_bytes([data[1], data[2]]) as usize;
            let mut offset = 3;
            let mut items = Vec::new();
            for _ in 0..len {
                if offset >= data.len() {
                    break;
                }
                let (item_len, item_str) = decode_cbor_item(&data[offset..], indent + 1);
                if item_len == 0 {
                    break;
                }
                items.push(format!("{}{}", spaces, item_str));
                offset += item_len;
            }
            (offset, format!("Array ({}):\n{}", len, items.join("\n")))
        }
        0xa0..=0xb7 => {
            let len = (first & 0x17) as usize;
            let mut offset = 1;
            let mut items = Vec::new();
            for _ in 0..len {
                if offset >= data.len() {
                    break;
                }
                let (key_len, key_str) = decode_cbor_item(&data[offset..], indent + 1);
                if key_len == 0 {
                    break;
                }
                offset += key_len;
                if offset >= data.len() {
                    break;
                }
                let (val_len, val_str) = decode_cbor_item(&data[offset..], indent + 1);
                if val_len == 0 {
                    break;
                }
                items.push(format!("{}{} -> {}", spaces, key_str, val_str));
                offset += val_len;
            }
            (offset, format!("Map ({}):\n{}", len, items.join("\n")))
        }
        0xb8 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            let len = data[1] as usize;
            let mut offset = 2;
            let mut items = Vec::new();
            for _ in 0..len {
                if offset >= data.len() {
                    break;
                }
                let (key_len, key_str) = decode_cbor_item(&data[offset..], indent + 1);
                if key_len == 0 {
                    break;
                }
                offset += key_len;
                if offset >= data.len() {
                    break;
                }
                let (val_len, val_str) = decode_cbor_item(&data[offset..], indent + 1);
                if val_len == 0 {
                    break;
                }
                items.push(format!("{}{} -> {}", spaces, key_str, val_str));
                offset += val_len;
            }
            (offset, format!("Map ({}):\n{}", len, items.join("\n")))
        }
        0xb9 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let len = u16::from_be_bytes([data[1], data[2]]) as usize;
            let mut offset = 3;
            let mut items = Vec::new();
            for _ in 0..len {
                if offset >= data.len() {
                    break;
                }
                let (key_len, key_str) = decode_cbor_item(&data[offset..], indent + 1);
                if key_len == 0 {
                    break;
                }
                offset += key_len;
                if offset >= data.len() {
                    break;
                }
                let (val_len, val_str) = decode_cbor_item(&data[offset..], indent + 1);
                if val_len == 0 {
                    break;
                }
                items.push(format!("{}{} -> {}", spaces, key_str, val_str));
                offset += val_len;
            }
            (offset, format!("Map ({}):\n{}", len, items.join("\n")))
        }
        0xc0 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            (2, format!("Tag: {}", data[1]))
        }
        0xc1..=0xdb => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            let tag = u64::from_be_bytes([0, 0, 0, 0, 0, 0, data[0] & 0x1f, data[1]]);
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let (inner_len, inner_str) = decode_cbor_item(&data[2..], indent);
            (2 + inner_len, format!("Tag({}) -> {}", tag, inner_str))
        }
        0xe0..=0xf3 => (1, format!("Simple: {}", first)),
        0xf4 => (1, "False".to_string()),
        0xf5 => (1, "True".to_string()),
        0xf6 => (1, "Null".to_string()),
        0xf7 => (1, "Undefined".to_string()),
        0xf8 => {
            if data.len() < 2 {
                return (0, "<incomplete>".to_string());
            }
            (2, format!("Simple: {}", data[1]))
        }
        0xf9 => {
            if data.len() < 3 {
                return (0, "<incomplete>".to_string());
            }
            let bits = u16::from_be_bytes([data[1], data[2]]);
            let (mantissa, exp) = decode_half_float(bits);
            (3, format!("Float16: {}", mantissa * 2.0_f64.powi(exp)))
        }
        0xfa => {
            if data.len() < 5 {
                return (0, "<incomplete>".to_string());
            }
            let val = f32::from_be_bytes([data[1], data[2], data[3], data[4]]);
            (5, format!("Float32: {}", val))
        }
        0xfb => {
            if data.len() < 9 {
                return (0, "<incomplete>".to_string());
            }
            let val = f64::from_be_bytes([
                data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
            ]);
            (9, format!("Float64: {}", val))
        }
        _ => (1, format!("Unknown: 0x{:02x}", first)),
    }
}

fn decode_half_float(bits: u16) -> (f64, i32) {
    let sign = (bits >> 15) & 1;
    let exp = (bits >> 10) & 0x1f;
    let mantissa = bits & 0x3ff;

    if exp == 0 {
        let val = mantissa as f64 * 2.0_f64.powi(-24);
        return (if sign == 1 { -val } else { val }, 0);
    }
    if exp == 31 {
        if mantissa == 0 {
            return (if sign == 1 { -1.0 } else { 1.0 }, i32::MAX);
        }
        return (f64::NAN, 0);
    }

    let val = (1024 + mantissa) as f64 * 2.0_f64.powi((exp as i32) - 15 - 10);
    (if sign == 1 { -val } else { val }, 0)
}
