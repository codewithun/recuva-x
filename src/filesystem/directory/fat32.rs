use anyhow::{Context, Result, bail};
use std::collections::HashSet;

use crate::disk::reader::DiskImageReader;
use crate::filesystem::directory::entries::{self, DirectoryEntry};
use crate::filesystem::metadata::fat32::{self, Fat32Metadata};

const SECTOR_SIZE: u64 = 512;
const FAT_ENTRY_SIZE: u64 = 4;

const FAT_MASK: u32 = 0x0FFF_FFFF;
const FAT_BAD_CLUSTER: u32 = 0x0FFF_FFF7;
const FAT_EOC_MIN: u32 = 0x0FFF_FFF8;
const FAT_RESERVED_MIN: u32 = 0x0FFF_FFF0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FatEntry {
    Next(u32),
    EndOfChain,
    Free,
    Bad,
    Reserved,
}

/// Read-only FAT32 cluster reader.
pub struct Fat32ClusterReader<'a> {
    reader: &'a mut DiskImageReader,
    metadata: Fat32Metadata,
    partition_offset: u64,
    fat_offset: u64,
    data_offset: u64,
}

impl<'a> Fat32ClusterReader<'a> {
    /// Initialize the reader using an existing FAT32 partition.
    pub fn new(
        reader: &'a mut DiskImageReader,
        start_lba: u64,
        partition_size: u64,
    ) -> Result<Self> {
        let partition_offset = start_lba
            .checked_mul(SECTOR_SIZE)
            .context("Partition offset overflow")?;

        let metadata = fat32::read(reader, start_lba, partition_size)?;

        let bytes_per_sector = u64::from(metadata.bytes_per_sector);

        let fat_offset = partition_offset
            .checked_add(
                u64::from(metadata.reserved_sectors)
                    .checked_mul(bytes_per_sector)
                    .context("FAT offset overflow")?,
            )
            .context("FAT offset overflow")?;

        let total_fat_sectors = u64::from(metadata.number_of_fats)
            .checked_mul(u64::from(metadata.fat_size_sectors))
            .context("Total FAT size overflow")?;

        let data_sector_offset = u64::from(metadata.reserved_sectors)
            .checked_add(total_fat_sectors)
            .context("Data region offset overflow")?;

        let data_offset = partition_offset
            .checked_add(
                data_sector_offset
                    .checked_mul(bytes_per_sector)
                    .context("Data offset overflow")?,
            )
            .context("Data offset overflow")?;

        Ok(Self {
            reader,
            metadata,
            partition_offset,
            fat_offset,
            data_offset,
        })
    }

    /// Return FAT32 filesystem metadata.
    pub fn metadata(&self) -> &Fat32Metadata {
        &self.metadata
    }

    /// Return the absolute byte offset of a cluster.
    fn cluster_offset(&self, cluster: u32) -> Result<u64> {
        self.validate_cluster(cluster)?;

        let cluster_index = u64::from(cluster - 2);

        let offset = cluster_index
            .checked_mul(self.metadata.cluster_size_bytes)
            .context("Cluster offset overflow")?;

        self.data_offset
            .checked_add(offset)
            .context("Cluster offset overflow")
    }

    /// Validate that a cluster belongs to the data region.
    fn validate_cluster(&self, cluster: u32) -> Result<()> {
        if cluster < 2 {
            bail!("Invalid FAT32 cluster number: {cluster}");
        }

        if u64::from(cluster) > self.metadata.data_cluster_count + 1 {
            bail!("Cluster {cluster} is outside FAT32 data boundaries");
        }

        Ok(())
    }

    /// Read one FAT entry and classify its value.
    pub fn read_fat_entry(&mut self, cluster: u32) -> Result<FatEntry> {
        self.validate_cluster(cluster)?;

        let entry_offset = u64::from(cluster)
            .checked_mul(FAT_ENTRY_SIZE)
            .context("FAT entry offset overflow")?;

        let fat_capacity_bytes = self
            .metadata
            .fat_capacity_entries
            .checked_mul(FAT_ENTRY_SIZE)
            .context("FAT capacity overflow")?;

        let entry_end = entry_offset
            .checked_add(FAT_ENTRY_SIZE)
            .context("FAT entry boundary overflow")?;

        if entry_end > fat_capacity_bytes {
            bail!("FAT entry exceeds FAT capacity");
        }

        let absolute_offset = self
            .fat_offset
            .checked_add(entry_offset)
            .context("FAT entry absolute offset overflow")?;

        let bytes = self.reader.read_bytes(absolute_offset, 4)?;

        let raw_value = u32::from_le_bytes(
            bytes
                .as_slice()
                .try_into()
                .context("Invalid FAT entry data")?,
        );

        let value = raw_value & FAT_MASK;

        match value {
            0 => Ok(FatEntry::Free),
            FAT_BAD_CLUSTER => Ok(FatEntry::Bad),
            FAT_EOC_MIN..=0x0FFF_FFFF => Ok(FatEntry::EndOfChain),
            FAT_RESERVED_MIN..=0x0FFF_FFF6 => Ok(FatEntry::Reserved),
            next if next >= 2 => {
                self.validate_cluster(next)?;
                Ok(FatEntry::Next(next))
            }
            _ => bail!("Invalid FAT32 cluster reference: {value:#010X}"),
        }
    }

    /// Read the contents of one cluster.
    pub fn read_cluster(&mut self, cluster: u32) -> Result<Vec<u8>> {
        let offset = self.cluster_offset(cluster)?;

        let length = usize::try_from(self.metadata.cluster_size_bytes)
            .context("Cluster size cannot fit into memory")?;

        self.reader
            .read_bytes(offset, length)
            .with_context(|| format!("Failed to read cluster {cluster}"))
    }

    /// Follow a cluster chain until End of Chain.
    ///
    /// Returns cluster numbers in traversal order.
    pub fn read_cluster_chain(&mut self, start_cluster: u32) -> Result<Vec<u32>> {
        self.validate_cluster(start_cluster)?;

        let mut chain = Vec::new();
        let mut visited = HashSet::new();

        let mut current = start_cluster;

        loop {
            if !visited.insert(current) {
                bail!("Circular FAT32 cluster chain detected at cluster {current}");
            }

            chain.push(current);

            if chain.len() as u64 > self.metadata.data_cluster_count {
                bail!("FAT32 cluster chain exceeds data cluster limit");
            }

            match self.read_fat_entry(current)? {
                FatEntry::Next(next) => {
                    current = next;
                }
                FatEntry::EndOfChain => {
                    break;
                }
                FatEntry::Free => {
                    bail!("Cluster {current} is marked as free");
                }
                FatEntry::Bad => {
                    bail!("Bad cluster encountered at cluster {current}");
                }
                FatEntry::Reserved => {
                    bail!("Reserved FAT entry encountered at cluster {current}");
                }
            }
        }

        Ok(chain)
    }

    /// Read and parse all entries from a FAT32 directory.
    ///
    /// The directory may span multiple clusters.
    pub fn read_directory(&mut self, start_cluster: u32) -> Result<Vec<DirectoryEntry>> {
        let cluster_chain = self.read_cluster_chain(start_cluster)?;

        let mut directory_data = Vec::new();

        for cluster in cluster_chain {
            let cluster_data = self.read_cluster(cluster)?;
            directory_data.extend_from_slice(&cluster_data);
        }

        entries::parse_directory_entries(&directory_data)
            .context("Failed to parse FAT32 directory entries")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    const START_LBA: u64 = 2;
    const PARTITION_SIZE: u64 = 1024 * 1024;

    fn create_test_image() -> Vec<u8> {
        let partition_offset = (START_LBA * 512) as usize;
        let mut image = vec![0u8; partition_offset + PARTITION_SIZE as usize];

        let boot = &mut image[partition_offset..partition_offset + 512];

        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 8;
        boot[14..16].copy_from_slice(&32u16.to_le_bytes());
        boot[16] = 2;
        boot[17..19].copy_from_slice(&0u16.to_le_bytes());
        boot[19..21].copy_from_slice(&0u16.to_le_bytes());
        boot[22..24].copy_from_slice(&0u16.to_le_bytes());
        boot[32..36].copy_from_slice(&2048u32.to_le_bytes());
        boot[36..40].copy_from_slice(&4u32.to_le_bytes());
        boot[44..48].copy_from_slice(&2u32.to_le_bytes());

        boot[510] = 0x55;
        boot[511] = 0xAA;

        image
    }

    fn write_fat_entry(image: &mut [u8], cluster: u32, value: u32) {
        let partition_offset = (START_LBA * 512) as usize;
        let fat_offset = partition_offset + (32 * 512);
        let entry_offset = fat_offset + (cluster * 4) as usize;

        image[entry_offset..entry_offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn run_test<T>(
        image: Vec<u8>,
        test: impl FnOnce(&mut Fat32ClusterReader<'_>) -> Result<T>,
    ) -> Result<T> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);

        let path = std::env::temp_dir().join(format!(
            "recuva-x-cluster-test-{}-{id}.img",
            std::process::id()
        ));

        fs::write(&path, image)?;

        let result = {
            let mut reader = DiskImageReader::open(&path)?;
            let mut cluster_reader =
                Fat32ClusterReader::new(&mut reader, START_LBA, PARTITION_SIZE)?;

            test(&mut cluster_reader)
        };

        let _ = fs::remove_file(&path);

        result
    }

    #[test]
    fn reads_next_cluster_entry() {
        let mut image = create_test_image();

        write_fat_entry(&mut image, 2, 3);

        let entry = run_test(image, |reader| reader.read_fat_entry(2)).unwrap();

        assert_eq!(entry, FatEntry::Next(3));
    }

    #[test]
    fn detects_end_of_chain() {
        let mut image = create_test_image();

        write_fat_entry(&mut image, 2, 0x0FFF_FFFF);

        let entry = run_test(image, |reader| reader.read_fat_entry(2)).unwrap();

        assert_eq!(entry, FatEntry::EndOfChain);
    }

    #[test]
    fn reads_cluster_content() {
        let mut image = create_test_image();

        let partition_offset = (START_LBA * 512) as usize;
        let cluster_offset = partition_offset + (40 * 512);

        image[cluster_offset..cluster_offset + 4].copy_from_slice(b"TEST");

        let content = run_test(image, |reader| reader.read_cluster(2)).unwrap();

        assert_eq!(&content[0..4], b"TEST");
        assert_eq!(content.len(), 4096);
    }

    #[test]
    fn follows_cluster_chain() {
        let mut image = create_test_image();

        write_fat_entry(&mut image, 2, 3);
        write_fat_entry(&mut image, 3, 0x0FFF_FFFF);

        let chain = run_test(image, |reader| reader.read_cluster_chain(2)).unwrap();

        assert_eq!(chain, vec![2, 3]);
    }

    #[test]
    fn rejects_circular_cluster_chain() {
        let mut image = create_test_image();

        write_fat_entry(&mut image, 2, 3);
        write_fat_entry(&mut image, 3, 2);

        let result = run_test(image, |reader| reader.read_cluster_chain(2));

        assert!(result.is_err());
    }

    #[test]
    fn rejects_cluster_outside_data_region() {
        let image = create_test_image();

        let result = run_test(image, |reader| reader.read_cluster(9999));

        assert!(result.is_err());
    }

    #[test]
    fn detects_free_cluster() {
        let image = create_test_image();

        let entry = run_test(image, |reader| reader.read_fat_entry(2)).unwrap();

        assert_eq!(entry, FatEntry::Free);
    }

    #[test]
    fn reads_directory_entries() {
        let mut image = create_test_image();

        write_fat_entry(&mut image, 2, 0x0FFF_FFFF);

        let partition_offset = (START_LBA * 512) as usize;
        let cluster_offset = partition_offset + (40 * 512);

        let mut entry = [0u8; 32];

        entry[0..11].copy_from_slice(b"HELLO   TXT");
        entry[11] = 0x20;
        entry[26..28].copy_from_slice(&5u16.to_le_bytes());
        entry[28..32].copy_from_slice(&1024u32.to_le_bytes());

        image[cluster_offset..cluster_offset + 32].copy_from_slice(&entry);

        let entries = run_test(image, |reader| reader.read_directory(2)).unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "HELLO.TXT");
        assert_eq!(entries[0].first_cluster, 5);
        assert_eq!(entries[0].size, 1024);
        assert!(entries[0].is_archive());
    }

    #[test]
    fn reads_directory_across_cluster_chain() {
        let mut image = create_test_image();

        write_fat_entry(&mut image, 2, 3);
        write_fat_entry(&mut image, 3, 0x0FFF_FFFF);

        let partition_offset = (START_LBA * 512) as usize;

        let cluster_2_offset = partition_offset + (40 * 512);
        let cluster_3_offset = partition_offset + (48 * 512);

        // Fill cluster 2 with deleted-entry markers.
        // This prevents the parser from stopping before cluster 3.
        let cluster_size = 4096;

        image[cluster_2_offset..cluster_2_offset + cluster_size].fill(0xE5);

        let mut first_entry = [0u8; 32];
        first_entry[0..11].copy_from_slice(b"FIRST   TXT");
        first_entry[11] = 0x20;

        let mut second_entry = [0u8; 32];
        second_entry[0..11].copy_from_slice(b"SECOND  TXT");
        second_entry[11] = 0x20;

        image[cluster_2_offset..cluster_2_offset + 32].copy_from_slice(&first_entry);

        image[cluster_3_offset..cluster_3_offset + 32].copy_from_slice(&second_entry);

        let entries = run_test(image, |reader| reader.read_directory(2)).unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "FIRST.TXT");
        assert_eq!(entries[1].name, "SECOND.TXT");
    }

    #[test]
    #[ignore = "Requires local test-images/test-fat32-directory.img"]
    fn reads_real_fat32_directory_image() {
        let path = std::path::Path::new("test-images/test-fat32-directory.img");

        let mut reader = DiskImageReader::open(path).unwrap();

        let mut directory_reader = Fat32ClusterReader::new(&mut reader, 2048, 102_400_000).unwrap();

        // Read root directory.
        let root_entries = directory_reader.read_directory(2).unwrap();

        for entry in &root_entries {
            println!(
                "ROOT: {} | cluster={} | size={} | directory={}",
                entry.name,
                entry.first_cluster,
                entry.size,
                entry.is_directory()
            );
        }

        assert!(root_entries.iter().any(|entry| entry.name == "hello.txt"));

        assert!(
            root_entries
                .iter()
                .any(|entry| entry.name == "notes with spaces.txt")
        );

        let documents = root_entries
            .iter()
            .find(|entry| entry.name == "Documents")
            .expect("Documents directory not found");

        assert!(documents.is_directory());

        // Read Documents subdirectory.
        let document_entries = directory_reader
            .read_directory(documents.first_cluster)
            .unwrap();

        for entry in &document_entries {
            println!(
                "DOCUMENTS: {} | cluster={} | size={}",
                entry.name, entry.first_cluster, entry.size
            );
        }

        assert!(
            document_entries
                .iter()
                .any(|entry| entry.name == "report.txt")
        );
    }
}
