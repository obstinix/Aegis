//! Linux x86-64 boot protocol: bzImage parsing, zero page, early page tables
//! and GDT (roadmap M3).
//!
//! Follows the kernel's `Documentation/arch/x86/boot.rst`. The 64-bit
//! protocol is used:
//! - the protected-mode kernel is loaded at 1 MiB;
//! - the zero page holds a copy of the image's setup header, patched with
//!   loader fields, plus an e820 map;
//! - the CPU enters at load address + 0x200 in long mode, with paging on,
//!   an identity map covering the kernel, zero page and command line,
//!   `__BOOT_CS`/`__BOOT_DS` flat segments, interrupts off, and RSI pointing
//!   at the zero page.
//!
//! Everything here is pure byte manipulation with bounds-checked offsets: a
//! malformed image is rejected with [`BootError`], never indexed out of range.

use std::fmt;

// Guest-physical layout used by Aegis (all inside the first 1 MiB except the kernel).
/// GDT location.
pub const GDT_ADDR: u64 = 0x500;
/// Early boot stack top.
pub const BOOT_STACK: u64 = 0x8FF0;
/// Zero page (`struct boot_params`).
pub const ZERO_PAGE_ADDR: u64 = 0x7000;
/// PML4 table.
pub const PML4_ADDR: u64 = 0x9000;
/// Page-directory-pointer table.
pub const PDPT_ADDR: u64 = 0xA000;
/// Page directory (512 x 2 MiB pages).
pub const PD_ADDR: u64 = 0xB000;
/// Kernel command line.
pub const CMDLINE_ADDR: u64 = 0x2_0000;
/// Protected-mode kernel load address (boot.rst: loaded high at 0x100000).
pub const KERNEL_ADDR: u64 = 0x10_0000;
/// End of usable low memory (start of the EBDA / legacy hole).
const LOW_RAM_END: u64 = 0x9_FC00;
/// The identity map covers this much (one page directory of 2 MiB pages).
pub const IDENTITY_MAP_BYTES: u64 = 1 << 30;

/// `__BOOT_CS`. The 64-bit boot protocol requires a GDT with flat descriptors
/// at `__BOOT_CS` (0x10, execute/read) and `__BOOT_DS` (0x18, read/write),
/// with CS = `__BOOT_CS` and DS/ES/SS = `__BOOT_DS`.
pub const CODE_SELECTOR: u16 = 0x10;
/// `__BOOT_DS`.
pub const DATA_SELECTOR: u16 = 0x18;
/// TSS selector. Not part of the protocol, but KVM requires a usable TR.
pub const TSS_SELECTOR: u16 = 0x20;
/// Size of the GDT returned by [`gdt`].
pub const GDT_SIZE: usize = 40;

// Setup header offsets in the image and in the zero page (boot.rst, "THE
// REAL-MODE KERNEL HEADER"). The zero page embeds the header at the same offsets.
const OFF_SETUP_SECTS: usize = 0x1F1;
const OFF_BOOT_FLAG: usize = 0x1FE;
const OFF_JUMP_LEN: usize = 0x201;
const OFF_HEADER: usize = 0x202;
const OFF_VERSION: usize = 0x206;
const OFF_TYPE_OF_LOADER: usize = 0x210;
const OFF_LOADFLAGS: usize = 0x211;
const OFF_RAMDISK_IMAGE: usize = 0x218;
const OFF_RAMDISK_SIZE: usize = 0x21C;
const OFF_CMD_LINE_PTR: usize = 0x228;
const OFF_INITRD_ADDR_MAX: usize = 0x22C;
const OFF_XLOADFLAGS: usize = 0x236;
const OFF_CMDLINE_SIZE: usize = 0x238;
const OFF_INIT_SIZE: usize = 0x260;
// Zero-page-only fields (boot.rst, "THE ZERO PAGE").
const OFF_E820_ENTRIES: usize = 0x1E8;
const OFF_E820_TABLE: usize = 0x2D0;
const E820_ENTRY_SIZE: usize = 20;
const E820_RAM: u32 = 1;

const BOOT_FLAG: u16 = 0xAA55;
const HEADER_MAGIC: &[u8; 4] = b"HdrS";
/// 2.12: first version with `xloadflags`, which says whether the 64-bit entry exists.
const MIN_VERSION: u16 = 0x020C;
const LOADED_HIGH: u8 = 0x01;
const XLF_KERNEL_64: u16 = 0x0001;
/// "Undefined" boot loader ID (boot.rst: 0xFF).
const LOADER_ID: u8 = 0xFF;
/// The setup header never extends past this offset in any protocol version.
const HEADER_LIMIT: usize = 0x290;
/// Size of the zero page.
pub const ZERO_PAGE_SIZE: usize = 4096;

/// Why a Linux boot could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootError {
    /// The image is too short or its header is malformed.
    BadImage(&'static str),
    /// The kernel needs a boot protocol feature this loader requires.
    Unsupported(String),
    /// The configuration does not fit the guest memory.
    DoesNotFit(String),
}

impl fmt::Display for BootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadImage(why) => write!(f, "not a usable bzImage: {why}"),
            Self::Unsupported(why) => write!(f, "unsupported kernel: {why}"),
            Self::DoesNotFit(why) => write!(f, "does not fit guest memory: {why}"),
        }
    }
}

impl std::error::Error for BootError {}

fn bytes<const N: usize>(data: &[u8], off: usize) -> Result<[u8; N], BootError> {
    let end = off
        .checked_add(N)
        .ok_or(BootError::BadImage("offset overflow"))?;
    data.get(off..end)
        .and_then(|s| s.try_into().ok())
        .ok_or(BootError::BadImage("image shorter than its setup header"))
}

fn put(page: &mut [u8], off: usize, value: &[u8]) {
    // Offsets are constants or bounds-checked below the zero page size.
    if let Some(dst) = off
        .checked_add(value.len())
        .and_then(|end| page.get_mut(off..end))
    {
        dst.copy_from_slice(value);
    }
}

/// A parsed bzImage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BzImage<'a> {
    image: &'a [u8],
    header_end: usize,
    kernel_offset: usize,
    /// Boot protocol version, e.g. 0x020F.
    pub version: u16,
    /// Memory the kernel needs from its load address while decompressing.
    pub init_size: u64,
    /// Highest address the initramfs may occupy.
    pub initrd_addr_max: u64,
    /// Longest command line accepted, excluding the NUL.
    pub cmdline_size: u64,
}

impl<'a> BzImage<'a> {
    /// Parse and validate a bzImage.
    ///
    /// # Errors
    ///
    /// [`BootError::BadImage`] for a truncated or malformed header;
    /// [`BootError::Unsupported`] for a protocol older than 2.12, a kernel
    /// that cannot be loaded high, or one without a 64-bit entry point.
    pub fn parse(image: &'a [u8]) -> Result<Self, BootError> {
        if u16::from_le_bytes(bytes(image, OFF_BOOT_FLAG)?) != BOOT_FLAG {
            return Err(BootError::BadImage("missing 0xAA55 boot flag"));
        }
        if &bytes::<4>(image, OFF_HEADER)? != HEADER_MAGIC {
            return Err(BootError::BadImage("missing HdrS header magic"));
        }
        let version = u16::from_le_bytes(bytes(image, OFF_VERSION)?);
        if version < MIN_VERSION {
            return Err(BootError::Unsupported(format!(
                "boot protocol {}.{:02} (need 2.12 or newer)",
                version >> 8,
                version & 0xFF
            )));
        }
        let [loadflags] = bytes::<1>(image, OFF_LOADFLAGS)?;
        if loadflags & LOADED_HIGH == 0 {
            return Err(BootError::Unsupported(
                "kernel cannot be loaded at 1 MiB".to_owned(),
            ));
        }
        if u16::from_le_bytes(bytes(image, OFF_XLOADFLAGS)?) & XLF_KERNEL_64 == 0 {
            return Err(BootError::Unsupported(
                "kernel has no 64-bit entry point".to_owned(),
            ));
        }

        // The header ends where the 2-byte jump at 0x200 lands.
        let [jump] = bytes::<1>(image, OFF_JUMP_LEN)?;
        let header_end = OFF_HEADER
            .checked_add(usize::from(jump))
            .ok_or(BootError::BadImage("bad jump"))?;
        if !(OFF_INIT_SIZE + 4..=HEADER_LIMIT).contains(&header_end) || header_end > image.len() {
            return Err(BootError::BadImage("setup header length out of range"));
        }

        // setup_sects == 0 means 4 (boot.rst). The real-mode code is
        // setup_sects + 1 sectors, including the boot sector.
        let [sects] = bytes::<1>(image, OFF_SETUP_SECTS)?;
        let sects = if sects == 0 { 4 } else { usize::from(sects) };
        let kernel_offset = sects
            .checked_add(1)
            .and_then(|s| s.checked_mul(512))
            .ok_or(BootError::BadImage("bad setup_sects"))?;
        if kernel_offset >= image.len() {
            return Err(BootError::BadImage(
                "no protected-mode kernel after the setup code",
            ));
        }

        Ok(Self {
            image,
            header_end,
            kernel_offset,
            version,
            init_size: u64::from(u32::from_le_bytes(bytes(image, OFF_INIT_SIZE)?)),
            initrd_addr_max: u64::from(u32::from_le_bytes(bytes(image, OFF_INITRD_ADDR_MAX)?)),
            cmdline_size: u64::from(u32::from_le_bytes(bytes(image, OFF_CMDLINE_SIZE)?)),
        })
    }

    /// The protected-mode kernel, to be loaded at [`KERNEL_ADDR`].
    #[must_use]
    pub fn protected_mode_kernel(&self) -> &'a [u8] {
        &self.image[self.kernel_offset..]
    }

    /// Address of the 64-bit entry point once loaded (boot.rst: load address + 0x200).
    #[must_use]
    pub const fn entry_64(&self) -> u64 {
        KERNEL_ADDR + 0x200
    }

    /// Lowest guest address above the kernel's working area.
    ///
    /// # Errors
    ///
    /// [`BootError::DoesNotFit`] on overflow.
    pub fn kernel_end(&self) -> Result<u64, BootError> {
        let len = u64::try_from(self.protected_mode_kernel().len())
            .map_err(|_| BootError::DoesNotFit("kernel size".to_owned()))?;
        KERNEL_ADDR
            .checked_add(len.max(self.init_size))
            .ok_or_else(|| BootError::DoesNotFit("kernel end overflows".to_owned()))
    }

    /// Choose where the initramfs goes: as high as RAM and `initrd_addr_max`
    /// allow, page-aligned, above the kernel's working area.
    ///
    /// # Errors
    ///
    /// [`BootError::DoesNotFit`] if it cannot be placed.
    pub fn place_initrd(&self, ram_bytes: u64, initrd_len: u64) -> Result<u64, BootError> {
        let top = ram_bytes.min(self.initrd_addr_max.saturating_add(1));
        let addr = top
            .checked_sub(initrd_len)
            .map(|a| a & !0xFFF)
            .ok_or_else(|| BootError::DoesNotFit(format!("initramfs of {initrd_len} bytes")))?;
        if addr < self.kernel_end()? {
            return Err(BootError::DoesNotFit(format!(
                "initramfs of {initrd_len} bytes overlaps the kernel; increase guest memory"
            )));
        }
        Ok(addr)
    }

    /// Build the zero page: the image's setup header, patched with loader
    /// fields, plus an e820 map describing `ram_bytes` of RAM.
    ///
    /// # Errors
    ///
    /// [`BootError::DoesNotFit`] if the command line is too long, RAM ends
    /// inside the kernel's working area, or an address does not fit the
    /// header's 32-bit fields.
    pub fn zero_page(
        &self,
        ram_bytes: u64,
        cmdline_len: u64,
        initrd: Option<(u64, u64)>,
    ) -> Result<[u8; ZERO_PAGE_SIZE], BootError> {
        if cmdline_len > self.cmdline_size {
            return Err(BootError::DoesNotFit(format!(
                "command line of {cmdline_len} bytes; kernel accepts {}",
                self.cmdline_size
            )));
        }
        if ram_bytes < self.kernel_end()? {
            return Err(BootError::DoesNotFit(
                "guest RAM ends inside the kernel".to_owned(),
            ));
        }
        let u32_field = |v: u64, what: &str| {
            u32::try_from(v).map_err(|_| BootError::DoesNotFit(format!("{what} above 4 GiB")))
        };

        let mut page = [0u8; ZERO_PAGE_SIZE];
        put(
            &mut page,
            OFF_SETUP_SECTS,
            &self.image[OFF_SETUP_SECTS..self.header_end],
        );
        put(&mut page, OFF_TYPE_OF_LOADER, &[LOADER_ID]);
        put(
            &mut page,
            OFF_CMD_LINE_PTR,
            &u32_field(CMDLINE_ADDR, "command line")?.to_le_bytes(),
        );
        if let Some((addr, len)) = initrd {
            put(
                &mut page,
                OFF_RAMDISK_IMAGE,
                &u32_field(addr, "initramfs")?.to_le_bytes(),
            );
            put(
                &mut page,
                OFF_RAMDISK_SIZE,
                &u32_field(len, "initramfs size")?.to_le_bytes(),
            );
        }

        // ram_bytes >= kernel_end() > KERNEL_ADDR, checked above.
        let high_ram = ram_bytes.saturating_sub(KERNEL_ADDR);
        put(&mut page, OFF_E820_ENTRIES, &[2]);
        for (entry, (addr, size)) in page[OFF_E820_TABLE..]
            .chunks_exact_mut(E820_ENTRY_SIZE)
            .zip([(0, LOW_RAM_END), (KERNEL_ADDR, high_ram)])
        {
            entry[..8].copy_from_slice(&addr.to_le_bytes());
            entry[8..16].copy_from_slice(&size.to_le_bytes());
            entry[16..].copy_from_slice(&E820_RAM.to_le_bytes());
        }
        Ok(page)
    }
}

/// The early page tables for [`PML4_ADDR`], [`PDPT_ADDR`] and [`PD_ADDR`]:
/// one page directory of 2 MiB pages identity-mapping the first
/// [`IDENTITY_MAP_BYTES`].
#[must_use]
pub fn identity_page_tables() -> [[u8; 4096]; 3] {
    const PRESENT_RW: u64 = 0x3;
    const PAGE_SIZE_2M: u64 = 0x80;
    let mut pages = [[0u8; 4096]; 3];
    put(&mut pages[0], 0, &(PDPT_ADDR | PRESENT_RW).to_le_bytes());
    put(&mut pages[1], 0, &(PD_ADDR | PRESENT_RW).to_le_bytes());
    for (i, entry) in pages[2].chunks_exact_mut(8).enumerate() {
        // i < 512, so i * 2 MiB < 1 GiB.
        let addr = u64::try_from(i).unwrap_or(0) << 21;
        entry.copy_from_slice(&(addr | PRESENT_RW | PAGE_SIZE_2M).to_le_bytes());
    }
    pages
}

/// The GDT: null, unused, `__BOOT_CS` (64-bit code), `__BOOT_DS` (data), and
/// a TSS descriptor, all flat with base 0.
#[must_use]
pub fn gdt() -> [u8; GDT_SIZE] {
    // 64-bit code: G=1, L=1, P=1, S=1, type 0xB (execute/read, accessed).
    // Data: G=1, D/B=1, P=1, S=1, type 0x3 (read/write, accessed).
    // TSS: G=1, P=1, S=0, type 0xB (busy 64-bit TSS).
    const ENTRIES: [u64; 5] = [
        0,
        0,
        0x00AF_9B00_0000_FFFF,
        0x00CF_9300_0000_FFFF,
        0x008F_8B00_0000_FFFF,
    ];
    let mut table = [0u8; GDT_SIZE];
    for (dst, entry) in table.chunks_exact_mut(8).zip(ENTRIES) {
        dst.copy_from_slice(&entry.to_le_bytes());
    }
    table
}

#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)] // fixed offsets into synthetic test images
mod tests {
    use super::*;

    /// A minimal synthetic bzImage: valid header, `setup_sects` = 1, and a
    /// 3-sector "kernel" after the setup code.
    fn image() -> Vec<u8> {
        let mut img = vec![0u8; 2 * 512 + 3 * 512];
        img[OFF_SETUP_SECTS] = 1;
        img[OFF_BOOT_FLAG..OFF_BOOT_FLAG + 2].copy_from_slice(&BOOT_FLAG.to_le_bytes());
        img[0x200] = 0xEB;
        img[OFF_JUMP_LEN] = 0x66; // header ends at 0x268
        img[OFF_HEADER..OFF_HEADER + 4].copy_from_slice(HEADER_MAGIC);
        img[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&0x020F_u16.to_le_bytes());
        img[OFF_LOADFLAGS] = LOADED_HIGH;
        img[OFF_XLOADFLAGS..OFF_XLOADFLAGS + 2].copy_from_slice(&XLF_KERNEL_64.to_le_bytes());
        img[OFF_INITRD_ADDR_MAX..OFF_INITRD_ADDR_MAX + 4]
            .copy_from_slice(&0x7FFF_FFFF_u32.to_le_bytes());
        img[OFF_CMDLINE_SIZE..OFF_CMDLINE_SIZE + 4].copy_from_slice(&2047_u32.to_le_bytes());
        img[OFF_INIT_SIZE..OFF_INIT_SIZE + 4].copy_from_slice(&0x40_0000_u32.to_le_bytes()); // 4 MiB
        img[2 * 512] = 0xAB; // first byte of the protected-mode kernel
        img
    }

    #[test]
    fn parses_a_valid_image() {
        let img = image();
        let bz = BzImage::parse(&img).unwrap();
        assert_eq!(bz.version, 0x020F);
        assert_eq!(bz.protected_mode_kernel().len(), 3 * 512);
        assert_eq!(bz.protected_mode_kernel()[0], 0xAB);
        assert_eq!(bz.entry_64(), 0x10_0200);
        assert_eq!(
            bz.kernel_end().unwrap(),
            0x10_0000 + 0x40_0000,
            "init_size dominates"
        );
    }

    #[test]
    fn zero_setup_sects_means_four() {
        let mut img = image();
        img[OFF_SETUP_SECTS] = 0;
        img.resize(5 * 512 + 1, 0);
        assert_eq!(
            BzImage::parse(&img).unwrap().protected_mode_kernel().len(),
            1
        );
    }

    #[test]
    fn rejects_malformed_and_unsupported_images() {
        let mutate = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut img = image();
            f(&mut img);
            BzImage::parse(&img).map(|_| ())
        };
        assert!(matches!(
            mutate(&|i| i[OFF_BOOT_FLAG] = 0),
            Err(BootError::BadImage(_))
        ));
        assert!(matches!(
            mutate(&|i| i[OFF_HEADER] = b'X'),
            Err(BootError::BadImage(_))
        ));
        assert!(matches!(
            mutate(&|i| i[OFF_VERSION] = 0x0B),
            Err(BootError::Unsupported(_))
        ));
        assert!(matches!(
            mutate(&|i| i[OFF_LOADFLAGS] = 0),
            Err(BootError::Unsupported(_))
        ));
        assert!(matches!(
            mutate(&|i| i[OFF_XLOADFLAGS] = 0),
            Err(BootError::Unsupported(_))
        ));
        assert!(matches!(
            mutate(&|i| i[OFF_JUMP_LEN] = 0x10),
            Err(BootError::BadImage(_))
        ));
        assert!(matches!(
            mutate(&|i| i.truncate(1024)),
            Err(BootError::BadImage(_))
        ));
        assert!(matches!(
            mutate(&|i| i.truncate(0x100)),
            Err(BootError::BadImage(_))
        ));
        assert!(BzImage::parse(&[]).is_err());
    }

    #[test]
    fn zero_page_copies_the_header_and_patches_loader_fields() {
        let img = image();
        let bz = BzImage::parse(&img).unwrap();
        let ram = 256 << 20;
        let initrd = (bz.place_initrd(ram, 10_000).unwrap(), 10_000);
        let zp = bz.zero_page(ram, 100, Some(initrd)).unwrap();

        assert_eq!(
            &zp[OFF_HEADER..OFF_HEADER + 4],
            HEADER_MAGIC,
            "header copied"
        );
        assert_eq!(zp[OFF_TYPE_OF_LOADER], LOADER_ID);
        assert_eq!(
            u32::from_le_bytes(bytes(&zp, OFF_CMD_LINE_PTR).unwrap()),
            0x2_0000
        );
        assert_eq!(
            u64::from(u32::from_le_bytes(bytes(&zp, OFF_RAMDISK_IMAGE).unwrap())),
            initrd.0
        );
        assert_eq!(
            u32::from_le_bytes(bytes(&zp, OFF_RAMDISK_SIZE).unwrap()),
            10_000
        );
        assert_eq!(zp[OFF_E820_ENTRIES], 2);
        let entry = |i: usize, f: usize| {
            u64::from_le_bytes(bytes(&zp, OFF_E820_TABLE + i * 20 + f).unwrap())
        };
        assert_eq!((entry(0, 0), entry(0, 8)), (0, 0x9_FC00));
        assert_eq!((entry(1, 0), entry(1, 8)), (0x10_0000, ram - 0x10_0000));
        assert_eq!(
            u32::from_le_bytes(bytes(&zp, OFF_E820_TABLE + 16).unwrap()),
            E820_RAM
        );
        assert!(
            zp[0x268..OFF_E820_TABLE].iter().all(|&b| b == 0),
            "nothing copied past the header"
        );
    }

    #[test]
    fn initrd_goes_high_page_aligned_and_never_over_the_kernel() {
        let img = image();
        let bz = BzImage::parse(&img).unwrap();
        let addr = bz.place_initrd(64 << 20, 5000).unwrap();
        assert_eq!(addr % 4096, 0);
        assert!(addr + 5000 <= 64 << 20);
        assert!(addr >= bz.kernel_end().unwrap());
        // 5 MiB of RAM leaves no room above the 4 MiB init_size at 1 MiB.
        assert!(matches!(
            bz.place_initrd(5 << 20, 0x20_0000),
            Err(BootError::DoesNotFit(_))
        ));
    }

    #[test]
    fn zero_page_rejects_what_does_not_fit() {
        let img = image();
        let bz = BzImage::parse(&img).unwrap();
        assert!(matches!(
            bz.zero_page(256 << 20, 2048, None),
            Err(BootError::DoesNotFit(_))
        ));
        assert!(matches!(
            bz.zero_page(2 << 20, 10, None),
            Err(BootError::DoesNotFit(_))
        ));
        assert!(bz.zero_page(256 << 20, 2047, None).is_ok());
    }

    #[test]
    fn page_tables_identity_map_the_first_gigabyte() {
        let [pml4, pdpt, pd] = identity_page_tables();
        assert_eq!(u64::from_le_bytes(pml4[..8].try_into().unwrap()), 0xA003);
        assert_eq!(u64::from_le_bytes(pdpt[..8].try_into().unwrap()), 0xB003);
        let pde = |i: usize| u64::from_le_bytes(pd[i * 8..i * 8 + 8].try_into().unwrap());
        assert_eq!(pde(0), 0x83);
        assert_eq!(pde(1), 0x20_0083);
        assert_eq!(pde(511), (511 << 21) | 0x83);
        assert!(pml4[8..].iter().chain(&pdpt[8..]).all(|&b| b == 0));
    }

    #[test]
    fn gdt_matches_the_boot_protocol_selectors() {
        let g = gdt();
        let e = |sel: u16| {
            let i = usize::from(sel);
            u64::from_le_bytes(g[i..i + 8].try_into().unwrap())
        };
        assert_eq!(e(0), 0);
        assert_eq!(e(0x08), 0, "unused slot");
        assert_eq!(
            (e(CODE_SELECTOR) >> 40) & 0xFF,
            0x9B,
            "__BOOT_CS: present, execute/read"
        );
        assert_eq!(
            (e(CODE_SELECTOR) >> 53) & 1,
            1,
            "__BOOT_CS: L bit, 64-bit code"
        );
        assert_eq!(
            (e(DATA_SELECTOR) >> 40) & 0xFF,
            0x93,
            "__BOOT_DS: present, read/write"
        );
        assert_eq!(
            (e(TSS_SELECTOR) >> 40) & 0xFF,
            0x8B,
            "present busy 64-bit TSS"
        );
    }
}
