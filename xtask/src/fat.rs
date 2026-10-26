//! A FAT16 image holding one file at `\EFI\BOOT\BOOTX64.EFI`, the path UEFI
//! firmware boots from on removable media (UEFI 2.10 §3.5.1.1).
//!
//! The volume has no partition table (a "superfloppy"), which the EDK II FAT
//! driver in OVMF accepts on a raw block device. The layout follows
//! Microsoft's FAT specification (fatgen103):
//! - a boot sector with the BIOS parameter block;
//! - two copies of the FAT;
//! - a fixed 512-entry root directory;
//! - 2 KiB clusters.
//!
//! The 16 MiB size gives 8167 clusters, so the volume is FAT16 (4085 to
//! 65524 clusters). Names are 8.3 only, so no long-name entries are needed.
//! The output is deterministic: fixed timestamps, volume ID and label.

use std::fmt;

/// Bytes per sector.
const BYTES_PER_SECTOR: u16 = 512;
/// Sectors in the image: 16 MiB.
const IMAGE_SECTORS: u16 = 32_768;
/// Image size in bytes.
pub(crate) const IMAGE_BYTES: usize = 16 * 1024 * 1024;
const SECTORS_PER_CLUSTER: u8 = 4;
const CLUSTER_BYTES: usize = 2048;
const RESERVED_SECTORS: u16 = 1;
const FATS: u8 = 2;
const ROOT_ENTRIES: u16 = 512;
/// Sectors per FAT, from fatgen103's formula for this geometry (checked by
/// a test).
const FAT_SECTORS: u16 = 32;
/// Byte offsets of the regions: one reserved sector, two FATs of 32
/// sectors, a 32-sector root directory, then data.
const FAT_START: usize = 512;
const FAT_BYTES: usize = 32 * 512;
const ROOT_START: usize = FAT_START + 2 * FAT_BYTES;
const DATA_START: usize = ROOT_START + 512 * 32;
/// Clusters in the data region: (32768 - 97 sectors) / 4.
const CLUSTERS: usize = 8167;
/// Cluster numbers: data clusters are numbered from 2.
const EFI_DIR: u16 = 2;
const BOOT_DIR: u16 = 3;
const FIRST_FILE_CLUSTER: u16 = 4;
/// The largest file that fits beside the two directory clusters.
pub(crate) const MAX_FILE_BYTES: usize = (CLUSTERS - 2) * CLUSTER_BYTES;

const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE: u8 = 0x20;
const MEDIA_FIXED: u8 = 0xF8;
const END_OF_CHAIN: u16 = 0xFFFF;
/// 2026-01-01 as a FAT date: `(2026 - 1980) << 9 | 1 << 5 | 1`.
const DATE: u16 = 0x5C21;

/// The file does not fit in the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TooLarge {
    /// Size of the file.
    pub(crate) bytes: usize,
}

impl fmt::Display for TooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bytes do not fit in the {IMAGE_BYTES}-byte FAT16 image (at most {MAX_FILE_BYTES})",
            self.bytes
        )
    }
}

impl std::error::Error for TooLarge {}

/// Copy `bytes` into `img` at `offset`. Every caller writes inside the
/// fixed layout above (sizes are checked up front), so running off the end
/// is a bug in this module.
fn put(img: &mut [u8], offset: usize, bytes: &[u8]) {
    img.get_mut(offset..)
        .and_then(|rest| rest.get_mut(..bytes.len()))
        .expect("write inside the image layout")
        .copy_from_slice(bytes);
}

fn boot_sector(img: &mut [u8]) {
    put(img, 0, &[0xEB, 0x3C, 0x90]); // jmp short to the (unused) boot code
    put(img, 3, b"AEGIS1.0");
    put(img, 11, &BYTES_PER_SECTOR.to_le_bytes());
    put(img, 13, &[SECTORS_PER_CLUSTER]);
    put(img, 14, &RESERVED_SECTORS.to_le_bytes());
    put(img, 16, &[FATS]);
    put(img, 17, &ROOT_ENTRIES.to_le_bytes());
    put(img, 19, &IMAGE_SECTORS.to_le_bytes());
    put(img, 21, &[MEDIA_FIXED]);
    put(img, 22, &FAT_SECTORS.to_le_bytes());
    put(img, 24, &32u16.to_le_bytes()); // sectors per track (unused)
    put(img, 26, &64u16.to_le_bytes()); // heads (unused)
    // 28: hidden sectors 0 (no partition table); 32: 32-bit total 0.
    put(img, 36, &[0x80, 0x00, 0x29]); // drive number, reserved, extended boot signature
    put(img, 39, b"AEGS"); // volume ID
    put(img, 43, b"AEGIS ESP  ");
    put(img, 54, b"FAT16   ");
    put(img, 62, &[0xEB, 0xFE]); // boot code: spin (never run by UEFI)
    put(img, 510, &[0x55, 0xAA]);
}

fn dir_entry(name: &[u8; 11], attr: u8, cluster: u16, size: u32) -> [u8; 32] {
    let mut e = [0u8; 32];
    e[..11].copy_from_slice(name);
    e[11] = attr;
    e[16..18].copy_from_slice(&DATE.to_le_bytes()); // creation date
    e[18..20].copy_from_slice(&DATE.to_le_bytes()); // last access date
    e[24..26].copy_from_slice(&DATE.to_le_bytes()); // write date
    e[26..28].copy_from_slice(&cluster.to_le_bytes());
    e[28..32].copy_from_slice(&size.to_le_bytes());
    e
}

/// Byte offset of data cluster `cluster` (numbered from 2).
fn cluster_offset(cluster: u16) -> usize {
    usize::from(cluster)
        .checked_sub(2)
        .and_then(|n| n.checked_mul(CLUSTER_BYTES))
        .and_then(|n| n.checked_add(DATA_START))
        .expect("a data cluster")
}

/// A FAT16 image whose only file is `efi`, at `\EFI\BOOT\BOOTX64.EFI`.
pub(crate) fn boot_image(efi: &[u8]) -> Result<Vec<u8>, TooLarge> {
    let too_large = TooLarge { bytes: efi.len() };
    if efi.len() > MAX_FILE_BYTES {
        return Err(too_large);
    }
    let size = u32::try_from(efi.len()).map_err(|_| too_large)?;
    let mut img = vec![0u8; IMAGE_BYTES];
    boot_sector(&mut img);

    // FAT: entry 0 holds the media byte, entry 1 the clean-shutdown bits;
    // each directory takes one cluster; the file is one contiguous chain,
    // each cluster pointing at the next and the last at end-of-chain. At
    // most MAX_FILE_BYTES / 2048 clusters, so the numbers stay below 2^16.
    let mut fat = vec![0u16; FAT_BYTES / 2];
    fat[0] = 0xFF00 | u16::from(MEDIA_FIXED);
    fat[1] = END_OF_CHAIN;
    fat[usize::from(EFI_DIR)] = END_OF_CHAIN;
    fat[usize::from(BOOT_DIR)] = END_OF_CHAIN;
    let chain: Vec<u16> = (FIRST_FILE_CLUSTER..=u16::MAX)
        .take(efi.len().div_ceil(CLUSTER_BYTES))
        .collect();
    for link in chain.windows(2) {
        fat[usize::from(link[0])] = link[1];
    }
    if let Some(&last) = chain.last() {
        fat[usize::from(last)] = END_OF_CHAIN;
    }
    let fat_bytes: Vec<u8> = fat.iter().flat_map(|e| e.to_le_bytes()).collect();
    put(&mut img, FAT_START, &fat_bytes);
    put(&mut img, FAT_START + FAT_BYTES, &fat_bytes);

    let root = [
        dir_entry(b"AEGIS ESP  ", ATTR_VOLUME_ID, 0, 0),
        dir_entry(b"EFI        ", ATTR_DIRECTORY, EFI_DIR, 0),
    ];
    put(&mut img, ROOT_START, &root.concat());
    // ".." of a top-level directory names the root as cluster 0.
    let efi_dir = [
        dir_entry(b".          ", ATTR_DIRECTORY, EFI_DIR, 0),
        dir_entry(b"..         ", ATTR_DIRECTORY, 0, 0),
        dir_entry(b"BOOT       ", ATTR_DIRECTORY, BOOT_DIR, 0),
    ];
    put(&mut img, cluster_offset(EFI_DIR), &efi_dir.concat());
    // An empty file has no clusters; its first cluster is 0.
    let start = chain.first().copied().unwrap_or(0);
    let boot_dir = [
        dir_entry(b".          ", ATTR_DIRECTORY, BOOT_DIR, 0),
        dir_entry(b"..         ", ATTR_DIRECTORY, EFI_DIR, 0),
        dir_entry(b"BOOTX64 EFI", ATTR_ARCHIVE, start, size),
    ];
    put(&mut img, cluster_offset(BOOT_DIR), &boot_dir.concat());

    // The chain is contiguous, so the file's bytes are too.
    put(&mut img, cluster_offset(FIRST_FILE_CLUSTER), efi);
    Ok(img)
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // offsets into a fixed-size test image
mod tests {
    use super::*;

    fn u16_at(b: &[u8], off: usize) -> u16 {
        u16::from_le_bytes([b[off], b[off + 1]])
    }

    fn u32_at(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }

    /// The layout as a FAT driver derives it from the boot sector, not from
    /// the writer's constants.
    struct Layout {
        bytes_per_sector: usize,
        cluster_bytes: usize,
        fat_start: usize,
        fat_bytes: usize,
        root_start: usize,
        root_entries: usize,
        data_start: usize,
        clusters: usize,
    }

    fn layout(img: &[u8]) -> Layout {
        assert_eq!(&img[510..512], &[0x55, 0xAA], "boot signature");
        let bps = usize::from(u16_at(img, 11));
        let spc = usize::from(img[13]);
        let reserved = usize::from(u16_at(img, 14));
        let fats = usize::from(img[16]);
        let root_entries = usize::from(u16_at(img, 17));
        let total = usize::from(u16_at(img, 19));
        let fat_sectors = usize::from(u16_at(img, 22));
        assert_eq!(u32_at(img, 32), 0, "16-bit sector count used");
        assert_eq!(img[38], 0x29, "extended BPB");
        assert_eq!(&img[54..62], b"FAT16   ");
        let root_sectors = (root_entries * 32).div_ceil(bps);
        let data_sector = reserved + fats * fat_sectors + root_sectors;
        Layout {
            bytes_per_sector: bps,
            cluster_bytes: bps * spc,
            fat_start: reserved * bps,
            fat_bytes: fat_sectors * bps,
            root_start: (reserved + fats * fat_sectors) * bps,
            root_entries,
            data_start: data_sector * bps,
            clusters: (total - data_sector) / spc,
        }
    }

    /// The entry named `name` among `count` entries at `at`.
    fn find(img: &[u8], at: usize, count: usize, name: &[u8; 11]) -> Option<(u8, u16, u32)> {
        (0..count)
            .map(|i| &img[at + i * 32..at + i * 32 + 32])
            .take_while(|e| e[0] != 0)
            .find(|e| &e[..11] == name)
            .map(|e| (e[11], u16_at(e, 26), u32_at(e, 28)))
    }

    /// Read `\EFI\BOOT\BOOTX64.EFI` back by walking directories and the FAT.
    fn read_back(img: &[u8]) -> Vec<u8> {
        let l = layout(img);
        let fat = &img[l.fat_start..l.fat_start + l.fat_bytes];
        assert_eq!(
            fat,
            &img[l.fat_start + l.fat_bytes..l.fat_start + 2 * l.fat_bytes],
            "both FAT copies are identical"
        );
        let cluster_at = |c: u16| l.data_start + usize::from(c - 2) * l.cluster_bytes;
        let per_cluster = l.cluster_bytes / 32;

        let (attr, efi, _) = find(img, l.root_start, l.root_entries, b"EFI        ").unwrap();
        assert_eq!(attr, ATTR_DIRECTORY);
        let (attr, boot, _) = find(img, cluster_at(efi), per_cluster, b"BOOT       ").unwrap();
        assert_eq!(attr, ATTR_DIRECTORY);
        let (attr, mut cluster, size) =
            find(img, cluster_at(boot), per_cluster, b"BOOTX64 EFI").unwrap();
        assert_eq!(attr, ATTR_ARCHIVE);

        let mut data = Vec::new();
        let mut steps = 0;
        while cluster != 0 && cluster < 0xFFF8 {
            assert!(usize::from(cluster) < l.clusters + 2, "cluster in range");
            data.extend_from_slice(
                &img[cluster_at(cluster)..cluster_at(cluster) + l.cluster_bytes],
            );
            cluster = u16_at(fat, usize::from(cluster) * 2);
            steps += 1;
            assert!(steps <= l.clusters, "no loop in the chain");
        }
        data.truncate(usize::try_from(size).unwrap());
        data
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| u8::try_from(i % 251).unwrap()).collect()
    }

    #[test]
    fn geometry_is_fat16_and_matches_the_specification_formula() {
        let img = boot_image(b"x").unwrap();
        let l = layout(&img);
        assert_eq!(img.len(), IMAGE_BYTES);
        assert_eq!(l.bytes_per_sector, 512);
        assert!(
            (4085..65525).contains(&l.clusters),
            "{} clusters is FAT16",
            l.clusters
        );
        assert_eq!(l.clusters, CLUSTERS);
        // fatgen103: FATSz = ceil((DskSize - (Rsvd + RootDirSectors)) /
        // (256 * SecPerClus + NumFATs)).
        let root_dir_sectors = (512 * 32usize).div_ceil(512);
        let fat_sz = (usize::from(IMAGE_SECTORS) - (1 + root_dir_sectors)).div_ceil(256 * 4 + 2);
        assert_eq!(fat_sz, usize::from(FAT_SECTORS));
        assert!(
            l.fat_bytes / 2 >= l.clusters + 2,
            "the FAT covers every cluster"
        );
        assert_eq!((l.root_start, l.data_start), (ROOT_START, DATA_START));
    }

    #[test]
    fn files_read_back_byte_for_byte() {
        for len in [
            1,
            100,
            CLUSTER_BYTES - 1,
            CLUSTER_BYTES,
            3 * CLUSTER_BYTES,
            1_000_003,
        ] {
            let file = pattern(len);
            assert_eq!(read_back(&boot_image(&file).unwrap()), file, "{len} bytes");
        }
    }

    #[test]
    fn the_largest_file_fits_and_one_more_byte_does_not() {
        let file = pattern(MAX_FILE_BYTES);
        assert_eq!(read_back(&boot_image(&file).unwrap()), file);
        assert_eq!(
            boot_image(&pattern(MAX_FILE_BYTES + 1)),
            Err(TooLarge {
                bytes: MAX_FILE_BYTES + 1
            })
        );
    }

    #[test]
    fn an_empty_file_has_no_clusters() {
        let img = boot_image(&[]).unwrap();
        assert!(read_back(&img).is_empty());
        let l = layout(&img);
        let boot_dir = l.data_start + usize::from(BOOT_DIR - 2) * l.cluster_bytes;
        assert_eq!(
            find(&img, boot_dir, 64, b"BOOTX64 EFI"),
            Some((ATTR_ARCHIVE, 0, 0))
        );
    }

    #[test]
    fn directories_link_back_and_the_volume_is_labelled() {
        let img = boot_image(b"efi").unwrap();
        let l = layout(&img);
        assert_eq!(
            find(&img, l.root_start, 512, b"AEGIS ESP  "),
            Some((ATTR_VOLUME_ID, 0, 0))
        );
        let efi = l.data_start;
        let boot = efi + l.cluster_bytes;
        assert_eq!(
            find(&img, efi, 64, b".          "),
            Some((ATTR_DIRECTORY, EFI_DIR, 0))
        );
        assert_eq!(
            find(&img, efi, 64, b"..         "),
            Some((ATTR_DIRECTORY, 0, 0)),
            "root is cluster 0"
        );
        assert_eq!(
            find(&img, boot, 64, b"..         "),
            Some((ATTR_DIRECTORY, EFI_DIR, 0))
        );
        let fat = &img[l.fat_start..];
        assert_eq!(u16_at(fat, 0), 0xFFF8, "media byte in FAT[0]");
        assert_eq!(u16_at(fat, 2), 0xFFFF);
    }

    #[test]
    fn output_is_deterministic() {
        let file = pattern(5000);
        assert_eq!(boot_image(&file).unwrap(), boot_image(&file).unwrap());
    }
}
