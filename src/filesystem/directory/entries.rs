use anyhow::{Result, bail};

const ENTRY_SIZE: usize = 32;
const LFN_ATTRIBUTE: u8 = 0x0F;

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN: u8 = 0x02;
const ATTR_SYSTEM: u8 = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub short_name: String,
    pub attributes: u8,
    pub first_cluster: u32,
    pub size: u32,
}

impl DirectoryEntry {
    pub fn is_directory(&self) -> bool {
        self.attributes & ATTR_DIRECTORY != 0
    }

    pub fn is_read_only(&self) -> bool {
        self.attributes & ATTR_READ_ONLY != 0
    }

    pub fn is_hidden(&self) -> bool {
        self.attributes & ATTR_HIDDEN != 0
    }

    pub fn is_system(&self) -> bool {
        self.attributes & ATTR_SYSTEM != 0
    }

    pub fn is_archive(&self) -> bool {
        self.attributes & ATTR_ARCHIVE != 0
    }
}

struct LfnState {
    checksum: u8,
    parts: Vec<Option<Vec<u16>>>,
}

impl LfnState {
    fn new(total: usize, checksum: u8) -> Self {
        Self {
            checksum,
            parts: vec![None; total],
        }
    }

    fn insert(&mut self, ordinal: usize, units: Vec<u16>) {
        if ordinal > 0 && ordinal <= self.parts.len() {
            self.parts[ordinal - 1] = Some(units);
        }
    }

    fn finish(self, short_name: &[u8; 11]) -> Option<String> {
        if self.checksum != lfn_checksum(short_name) {
            return None;
        }

        let mut units = Vec::new();

        for part in self.parts {
            units.extend(part?);
        }

        while matches!(units.last(), Some(0x0000 | 0xFFFF)) {
            units.pop();
        }

        if units.is_empty() {
            return None;
        }

        if units.contains(&0x0000) {
            return None;
        }

        Some(String::from_utf16_lossy(&units))
    }
}

fn lfn_checksum(short_name: &[u8; 11]) -> u8 {
    short_name
        .iter()
        .fold(0u8, |sum, byte| sum.rotate_right(1).wrapping_add(*byte))
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn decode_short_name(data: &[u8]) -> String {
    let mut name = data[0..8].to_vec();
    let extension = &data[8..11];

    if name[0] == 0x05 {
        name[0] = 0xE5;
    }

    let base = String::from_utf8_lossy(&name).trim_end().to_string();

    let ext = String::from_utf8_lossy(extension).trim_end().to_string();

    if ext.is_empty() {
        base
    } else {
        format!("{base}.{ext}")
    }
}

fn extract_lfn_units(entry: &[u8]) -> Vec<u16> {
    let offsets = [(1, 10), (14, 12), (28, 4)];

    let mut units = Vec::with_capacity(13);

    for (offset, length) in offsets {
        for position in (offset..offset + length).step_by(2) {
            units.push(read_u16(entry, position));
        }
    }

    units
}

/// Parse FAT32 directory entries from raw directory cluster data.
pub fn parse_directory_entries(data: &[u8]) -> Result<Vec<DirectoryEntry>> {
    if !data.len().is_multiple_of(ENTRY_SIZE) {
        bail!("Directory data length is not a multiple of 32 bytes");
    }

    let mut entries = Vec::new();
    let mut lfn_state: Option<LfnState> = None;

    for entry in data.chunks_exact(ENTRY_SIZE) {
        let first_byte = entry[0];

        // End of directory.
        if first_byte == 0x00 {
            break;
        }

        // Deleted entry.
        if first_byte == 0xE5 {
            lfn_state = None;
            continue;
        }

        let attributes = entry[11];

        // Long File Name entry.
        if attributes == LFN_ATTRIBUTE {
            let ordinal_byte = entry[0];
            let ordinal = usize::from(ordinal_byte & 0x1F);
            let is_last = ordinal_byte & 0x40 != 0;

            if ordinal == 0 || ordinal > 20 || entry[12] != 0 || read_u16(entry, 26) != 0 {
                lfn_state = None;
                continue;
            }

            if is_last {
                lfn_state = Some(LfnState::new(ordinal, entry[13]));
            }

            if let Some(state) = lfn_state.as_mut() {
                if state.checksum != entry[13]
                    || ordinal > state.parts.len()
                    || state.parts[ordinal - 1].is_some()
                {
                    lfn_state = None;
                    continue;
                }

                state.insert(ordinal, extract_lfn_units(entry));
            } else {
                lfn_state = None;
            }

            continue;
        }

        let short_name_bytes: [u8; 11] = entry[0..11].try_into()?;

        let short_name = decode_short_name(&short_name_bytes);

        // Volume labels are not ordinary files or directories.
        if attributes & ATTR_VOLUME_ID != 0 {
            lfn_state = None;
            continue;
        }

        // Skip special directory references.
        if short_name == "." || short_name == ".." {
            lfn_state = None;
            continue;
        }

        let long_name = lfn_state
            .take()
            .and_then(|state| state.finish(&short_name_bytes));

        let name = long_name.unwrap_or_else(|| short_name.clone());

        let high_cluster = u32::from(read_u16(entry, 20));
        let low_cluster = u32::from(read_u16(entry, 26));

        let first_cluster = (high_cluster << 16) | low_cluster;

        let size = read_u32(entry, 28);

        entries.push(DirectoryEntry {
            name,
            short_name,
            attributes,
            first_cluster,
            size,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_entry(name: &[u8; 11], attributes: u8, cluster: u32, size: u32) -> [u8; ENTRY_SIZE] {
        let mut entry = [0u8; ENTRY_SIZE];

        entry[0..11].copy_from_slice(name);
        entry[11] = attributes;

        entry[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());

        entry[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());

        entry[28..32].copy_from_slice(&size.to_le_bytes());

        entry
    }

    #[test]
    fn parses_short_filename() {
        let entry = create_entry(b"HELLO   TXT", ATTR_ARCHIVE, 5, 1024);

        let entries = parse_directory_entries(&entry).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "HELLO.TXT");
        assert_eq!(entries[0].first_cluster, 5);
        assert_eq!(entries[0].size, 1024);
        assert!(entries[0].is_archive());
    }

    #[test]
    fn parses_directory_attribute() {
        let entry = create_entry(b"DOCUMENTS  ", ATTR_DIRECTORY, 10, 0);

        let entries = parse_directory_entries(&entry).unwrap();

        assert_eq!(entries.len(), 1);
        assert!(entries[0].is_directory());
        assert_eq!(entries[0].first_cluster, 10);
    }

    #[test]
    fn skips_deleted_entries() {
        let mut entry = create_entry(b"DELETED TXT", ATTR_ARCHIVE, 5, 100);

        entry[0] = 0xE5;

        let entries = parse_directory_entries(&entry).unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn stops_at_end_marker() {
        let first = create_entry(b"FIRST   TXT", ATTR_ARCHIVE, 2, 100);

        let second = create_entry(b"SECOND  TXT", ATTR_ARCHIVE, 3, 200);

        let mut data = Vec::new();

        data.extend_from_slice(&first);
        data.extend_from_slice(&[0u8; ENTRY_SIZE]);
        data.extend_from_slice(&second);

        let entries = parse_directory_entries(&data).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "FIRST.TXT");
    }

    #[test]
    fn rejects_invalid_directory_data_length() {
        let data = vec![0u8; 31];

        assert!(parse_directory_entries(&data).is_err());
    }

    #[test]
    fn parses_long_filename() {
        let short_name = *b"LONGNA~1TXT";
        let long_name = "Long Filename.txt";

        let encoded: Vec<u16> = long_name.encode_utf16().collect();

        let total_parts = encoded.len().div_ceil(13);
        let checksum = lfn_checksum(&short_name);

        let mut data = Vec::new();

        for ordinal in (1..=total_parts).rev() {
            let mut lfn = [0xFFu8; ENTRY_SIZE];

            lfn[0] = ordinal as u8;

            if ordinal == total_parts {
                lfn[0] |= 0x40;
            }

            lfn[11] = LFN_ATTRIBUTE;
            lfn[12] = 0;
            lfn[13] = checksum;
            lfn[26..28].copy_from_slice(&0u16.to_le_bytes());

            let start = (ordinal - 1) * 13;

            for index in 0..13 {
                let unit = encoded.get(start + index).copied().unwrap_or(0xFFFF);

                let offset = match index {
                    0..=4 => 1 + index * 2,
                    5..=10 => 14 + (index - 5) * 2,
                    _ => 28 + (index - 11) * 2,
                };

                lfn[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
            }

            data.extend_from_slice(&lfn);
        }

        let short_entry = create_entry(&short_name, ATTR_ARCHIVE, 7, 2048);

        data.extend_from_slice(&short_entry);

        let entries = parse_directory_entries(&data).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, long_name);
        assert_eq!(entries[0].short_name, "LONGNA~1.TXT");
    }
}
