import struct
import uuid
import zlib

SECTOR_SIZE = 512
TOTAL_SECTORS = 4096
IMAGE_SIZE = TOTAL_SECTORS * SECTOR_SIZE

image = bytearray(IMAGE_SIZE)

# Protective MBR
image[446] = 0x00
image[450] = 0xEE
struct.pack_into("<I", image, 454, 1)
struct.pack_into("<I", image, 458, TOTAL_SECTORS - 1)
image[510:512] = b"\x55\xAA"

# GPT partition entries
ENTRY_COUNT = 128
ENTRY_SIZE = 128
TABLE_SIZE = ENTRY_COUNT * ENTRY_SIZE

entries = bytearray(TABLE_SIZE)

# Partition 1: Linux filesystem
type_guid = uuid.UUID("0FC63DAF-8483-4772-8E79-3D69D8477DE4")
unique_guid = uuid.uuid4()

entry = bytearray(ENTRY_SIZE)
entry[0:16] = type_guid.bytes_le
entry[16:32] = unique_guid.bytes_le
struct.pack_into("<Q", entry, 32, 2048)
struct.pack_into("<Q", entry, 40, 3071)

name = "Linux Data".encode("utf-16le")
entry[56:56 + len(name)] = name

entries[0:ENTRY_SIZE] = entry

entries_crc = zlib.crc32(entries) & 0xFFFFFFFF

# Primary and backup partition tables
PRIMARY_ENTRIES_LBA = 2
TABLE_SECTORS = TABLE_SIZE // SECTOR_SIZE
BACKUP_ENTRIES_LBA = TOTAL_SECTORS - 1 - TABLE_SECTORS

image[
    PRIMARY_ENTRIES_LBA * SECTOR_SIZE:
    PRIMARY_ENTRIES_LBA * SECTOR_SIZE + TABLE_SIZE
] = entries

image[
    BACKUP_ENTRIES_LBA * SECTOR_SIZE:
    BACKUP_ENTRIES_LBA * SECTOR_SIZE + TABLE_SIZE
] = entries

disk_guid = uuid.uuid4()

def make_header(current_lba, backup_lba, entries_lba):
    header = bytearray(SECTOR_SIZE)

    header[0:8] = b"EFI PART"
    struct.pack_into("<I", header, 8, 0x00010000)
    struct.pack_into("<I", header, 12, 92)
    struct.pack_into("<I", header, 16, 0)
    struct.pack_into("<I", header, 20, 0)

    struct.pack_into("<Q", header, 24, current_lba)
    struct.pack_into("<Q", header, 32, backup_lba)
    struct.pack_into("<Q", header, 40, 34)
    struct.pack_into("<Q", header, 48, TOTAL_SECTORS - 34 - 1)

    header[56:72] = disk_guid.bytes_le

    struct.pack_into("<Q", header, 72, entries_lba)
    struct.pack_into("<I", header, 80, ENTRY_COUNT)
    struct.pack_into("<I", header, 84, ENTRY_SIZE)
    struct.pack_into("<I", header, 88, entries_crc)

    header_crc = zlib.crc32(header[:92]) & 0xFFFFFFFF
    struct.pack_into("<I", header, 16, header_crc)

    return header

# Primary GPT header
image[SECTOR_SIZE:2 * SECTOR_SIZE] = make_header(
    1, TOTAL_SECTORS - 1, PRIMARY_ENTRIES_LBA
)

# Backup GPT header
image[
    (TOTAL_SECTORS - 1) * SECTOR_SIZE:
    TOTAL_SECTORS * SECTOR_SIZE
] = make_header(
    TOTAL_SECTORS - 1, 1, BACKUP_ENTRIES_LBA
)

with open("test-gpt-partition.img", "wb") as f:
    f.write(image)

print("GPT test image created successfully.")
print(f"Size: {IMAGE_SIZE} bytes")
print("Partitions: 1")
print("Partition: Linux Data")
print("First LBA: 2048")
print("Last LBA: 3071")
