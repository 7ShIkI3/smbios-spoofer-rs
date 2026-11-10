//! Windows Kernel Storage IOCTL buffer patchers (`no_std`).
//!
//! Shared between the UEFI/host test suite and the `smbios-disk-filter.sys`
//! Windows kernel filter driver (`driver/`).
//!
//! Handles all 5 Windows disk serial query vectors:
//! 1. `IOCTL_STORAGE_QUERY_PROPERTY` (`0x002D1400`):
//!    - `StorageDeviceProperty (0)` -> `STORAGE_DEVICE_DESCRIPTOR`
//!    - `StorageDeviceIdProperty (2)` -> `STORAGE_DEVICE_ID_DESCRIPTOR` (VPD 0x83)
//!    - `StorageAdapterProtocolSpecificProperty (49)` / `StorageDeviceProtocolSpecificProperty (50)`
//!      -> `STORAGE_PROTOCOL_DATA_DESCRIPTOR` (NVMe 4096-byte Identify Controller & ATA 512-byte Identify)
//!    - `StorageAdapterSerialNumberProperty (56)` -> `STORAGE_ADAPTER_SERIAL_NUMBER` (UTF-16LE)
//! 2. `IOCTL_STORAGE_PROTOCOL_COMMAND` (`0x002D14E0` / `0x002D53C0`)
//! 3. `SMART_RCV_DRIVE_DATA` (`0x0007C088`) -> `SENDCMDOUTPARAMS` (ATA word-swapped serial)
//! 4. `IOCTL_ATA_PASS_THROUGH` (`0x0004D02C`) -> `ATA_PASS_THROUGH_EX`
//! 5. `IOCTL_SCSI_PASS_THROUGH` (`0x0004D004`) / SCSI `INQUIRY` VPD Page `0x80`

/// `IOCTL_STORAGE_QUERY_PROPERTY` (`CTL_CODE(IOCTL_STORAGE_BASE, 0x0500, METHOD_BUFFERED, FILE_ANY_ACCESS)`)
pub const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;

/// `IOCTL_STORAGE_PROTOCOL_COMMAND` (`CTL_CODE(IOCTL_STORAGE_BASE, 0x04F0, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)`)
pub const IOCTL_STORAGE_PROTOCOL_COMMAND: u32 = 0x002D_D3C0;
/// Alternate read-only / any-access code sometimes sent for protocol commands
pub const IOCTL_STORAGE_PROTOCOL_COMMAND_ALT: u32 = 0x002D_13C0;

/// `SMART_RCV_DRIVE_DATA` (`CTL_CODE(IOCTL_DISK_BASE, 0x0022, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)`)
pub const SMART_RCV_DRIVE_DATA: u32 = 0x0007_C088;

/// `IOCTL_ATA_PASS_THROUGH` (`CTL_CODE(IOCTL_SCSI_BASE, 0x040B, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)`)
pub const IOCTL_ATA_PASS_THROUGH: u32 = 0x0004_D02C;

/// `IOCTL_SCSI_PASS_THROUGH` (`CTL_CODE(IOCTL_SCSI_BASE, 0x0401, METHOD_BUFFERED, FILE_READ_ACCESS | FILE_WRITE_ACCESS)`)
pub const IOCTL_SCSI_PASS_THROUGH: u32 = 0x0004_D004;

// `STORAGE_PROPERTY_ID` values from `ntddstor.h`
pub const STORAGE_DEVICE_PROPERTY: u32 = 0;
pub const STORAGE_ADAPTER_PROPERTY: u32 = 1;
pub const STORAGE_DEVICE_ID_PROPERTY: u32 = 2;
pub const STORAGE_DEVICE_UNIQUE_ID_PROPERTY: u32 = 3;
pub const STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY: u32 = 49;
pub const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;
pub const STORAGE_ADAPTER_SERIAL_NUMBER_PROPERTY: u32 = 56;
pub const STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY_EX: u32 = 67;
pub const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY_EX: u32 = 68;

// `STORAGE_QUERY_TYPE`
pub const PROPERTY_STANDARD_QUERY: u32 = 0;

// `STORAGE_PROTOCOL_TYPE`
pub const PROTOCOL_TYPE_ATA: u32 = 2;
pub const PROTOCOL_TYPE_NVME: u32 = 3;

// `STORAGE_PROTOCOL_NVME_DATA_TYPE` / `STORAGE_PROTOCOL_ATA_DATA_TYPE`
pub const PROTOCOL_DATA_TYPE_IDENTIFY: u32 = 1;

// Completion context tags packed into the upper 32 bits of `Context`
pub const TAG_NONE: u8 = 0;
pub const TAG_STORAGE_DEVICE_DESCRIPTOR: u8 = 1;
pub const TAG_STORAGE_DEVICE_ID_DESCRIPTOR: u8 = 2;
pub const TAG_STORAGE_PROTOCOL_DESCRIPTOR: u8 = 3;
pub const TAG_STORAGE_ADAPTER_SERIAL: u8 = 4;
pub const TAG_STORAGE_PROTOCOL_COMMAND: u8 = 5;
pub const TAG_SMART_RCV_DRIVE_DATA: u8 = 6;
pub const TAG_ATA_PASS_THROUGH: u8 = 7;
pub const TAG_SCSI_PASS_THROUGH: u8 = 8;
pub const TAG_STORAGE_DEVICE_UNIQUE_ID: u8 = 9;
pub const TAG_GENERIC_SUBSTRING_SCAN: u8 = 10;

/// Inspects an incoming `IRP_MJ_DEVICE_CONTROL` request (`io_control_code` and `system_buffer`)
/// BEFORE the lower driver overwrites `SystemBuffer`, and returns the completion tag (`TAG_*`)
/// to use when the IRP completes.
pub fn classify_ioctl_request(io_control_code: u32, input_buf: &[u8]) -> u8 {
    match io_control_code {
        IOCTL_STORAGE_QUERY_PROPERTY => {
            if input_buf.len() < 8 {
                return TAG_NONE;
            }
            let property_id =
                u32::from_le_bytes([input_buf[0], input_buf[1], input_buf[2], input_buf[3]]);
            let query_type =
                u32::from_le_bytes([input_buf[4], input_buf[5], input_buf[6], input_buf[7]]);
            if query_type != PROPERTY_STANDARD_QUERY {
                return TAG_NONE;
            }
            match property_id {
                STORAGE_DEVICE_PROPERTY => TAG_STORAGE_DEVICE_DESCRIPTOR,
                STORAGE_DEVICE_ID_PROPERTY => TAG_STORAGE_DEVICE_ID_DESCRIPTOR,
                STORAGE_DEVICE_UNIQUE_ID_PROPERTY => TAG_STORAGE_DEVICE_UNIQUE_ID,
                STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY
                | STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY
                | STORAGE_ADAPTER_PROTOCOL_SPECIFIC_PROPERTY_EX
                | STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY_EX => TAG_STORAGE_PROTOCOL_DESCRIPTOR,
                STORAGE_ADAPTER_SERIAL_NUMBER_PROPERTY => TAG_STORAGE_ADAPTER_SERIAL,
                _ => TAG_GENERIC_SUBSTRING_SCAN,
            }
        }
        IOCTL_STORAGE_PROTOCOL_COMMAND | IOCTL_STORAGE_PROTOCOL_COMMAND_ALT => {
            TAG_STORAGE_PROTOCOL_COMMAND
        }
        SMART_RCV_DRIVE_DATA => TAG_SMART_RCV_DRIVE_DATA,
        IOCTL_ATA_PASS_THROUGH => TAG_ATA_PASS_THROUGH,
        IOCTL_SCSI_PASS_THROUGH => TAG_SCSI_PASS_THROUGH,
        _ => {
            // If METHOD_BUFFERED (low 2 bits == 0), allow substring scan on completion
            if (io_control_code & 0x3) == 0 {
                TAG_GENERIC_SUBSTRING_SCAN
            } else {
                TAG_NONE
            }
        }
    }
}

/// Dispatches the appropriate in-place buffer patcher based on `tag`.
pub fn patch_ioctl_output_by_tag(
    tag: u8,
    buf: &mut [u8],
    new_serial: &[u8; 20],
    seed: u64,
) -> bool {
    match tag {
        TAG_STORAGE_DEVICE_DESCRIPTOR => patch_storage_device_descriptor(buf, new_serial),
        TAG_STORAGE_DEVICE_ID_DESCRIPTOR => {
            patch_storage_device_id_descriptor(buf, new_serial, seed)
        }
        TAG_STORAGE_PROTOCOL_DESCRIPTOR => patch_storage_protocol_descriptor(buf, new_serial),
        TAG_STORAGE_ADAPTER_SERIAL => patch_storage_adapter_serial_number(buf, new_serial),
        TAG_STORAGE_PROTOCOL_COMMAND => patch_storage_protocol_command(buf, new_serial),
        TAG_SMART_RCV_DRIVE_DATA => patch_smart_rcv_drive_data(buf, new_serial),
        TAG_ATA_PASS_THROUGH => patch_ata_pass_through(buf, new_serial),
        TAG_SCSI_PASS_THROUGH => patch_scsi_pass_through(buf, new_serial),
        _ => false,
    }
}

/// Patches an ASCII serial number slice in-place while preserving:
/// - `stornvme.sys` namespace suffixes such as `"   _00000001."` (`20-byte SN` + `_` + `8-hex NSID` + `.`)
/// - Leading/trailing ASCII space padding (`0x20`)
/// - Hex-encoded serials (when `disk.sys` hex-encodes a 20-byte serial into 40 hex chars)
pub fn patch_ascii_serial_slice(serial_slice: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if serial_slice.is_empty() {
        return false;
    }

    // Case 1: Check for `stornvme.sys` VPD 0x80 synthesized format:
    // `[20-byte Controller SN]_[8-char hex NSID].` (30 bytes total, e.g. `CDA7N706410107T15   _00000001.`)
    if let Some(underscore_pos) = find_stornvme_nsid_suffix(serial_slice) {
        let prefix = &mut serial_slice[..underscore_pos];
        overwrite_preserving_spaces(prefix, new_serial);
        return true;
    }

    // Case 2: 40-char hex-encoded serial (used by `disk.sys` for some SATA/SCSI drives)
    if serial_slice.len() == 40 && serial_slice.iter().all(|&b| b.is_ascii_hexdigit()) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for (i, &b) in new_serial.iter().enumerate() {
            serial_slice[i * 2] = HEX[(b >> 4) as usize];
            serial_slice[i * 2 + 1] = HEX[(b & 0x0F) as usize];
        }
        return true;
    }

    // Case 3: Standard ASCII serial string
    overwrite_preserving_spaces(serial_slice, new_serial);
    true
}

/// Detects the `_00000001.` suffix synthesized by `stornvme.sys` (`_<8 hex digits>.`)
/// and returns the index of the `_` separator.
fn find_stornvme_nsid_suffix(s: &[u8]) -> Option<usize> {
    // Look for `_XXXXXXXX.` at the end of the string (10 bytes: `_` + 8 hex digits + `.`)
    if s.len() < 22 {
        return None;
    }
    let suffix_start = s.len() - 10;
    let suffix = &s[suffix_start..];
    if suffix[0] == b'_'
        && suffix[9] == b'.'
        && suffix[1..9].iter().all(|&c| c.is_ascii_hexdigit())
    {
        return Some(suffix_start);
    }
    None
}

/// Overwrites `target` with characters from `new_serial` while preserving leading and trailing
/// spaces (`0x20`) in `target`.
fn overwrite_preserving_spaces(target: &mut [u8], new_serial: &[u8; 20]) {
    let mut src_idx = 0usize;
    for dst in target.iter_mut() {
        if *dst == b' ' {
            continue;
        }
        // Advance `src_idx` to the next non-space printable character in `new_serial`
        while src_idx < new_serial.len() && new_serial[src_idx] == b' ' {
            src_idx += 1;
        }
        if src_idx < new_serial.len() {
            *dst = new_serial[src_idx];
            src_idx += 1;
        } else {
            // Wrap around non-space characters if target is longer than 20 bytes
            let fallback = new_serial[0];
            if fallback != b' ' {
                *dst = fallback;
            }
        }
    }
}

/// 1A. Patches `STORAGE_DEVICE_DESCRIPTOR` (`IOCTL_STORAGE_QUERY_PROPERTY`, `PropertyId = 0`).
///
/// Layout (`ntddstor.h`):
/// - `0..4`: `Version: u32`
/// - `4..8`: `Size: u32`
/// - `24..28`: `SerialNumberOffset: u32` (byte offset to null-terminated ASCII string)
pub fn patch_storage_device_descriptor(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if buf.len() < 36 {
        return false;
    }
    let serial_offset = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]) as usize;
    if serial_offset < 28 || serial_offset >= buf.len() {
        return false;
    }

    // Find null terminator starting at `serial_offset`
    let max_rem = buf.len() - serial_offset;
    let mut str_len = 0usize;
    while str_len < max_rem && buf[serial_offset + str_len] != 0 {
        str_len += 1;
    }
    if str_len == 0 {
        return false;
    }

    patch_ascii_serial_slice(
        &mut buf[serial_offset..serial_offset + str_len],
        new_serial,
    )
}

/// 1B. Patches `STORAGE_DEVICE_ID_DESCRIPTOR` (`IOCTL_STORAGE_QUERY_PROPERTY`, `PropertyId = 2`).
///
/// Layout (`ntddstor.h`):
/// - `0..4`: `Version: u32`
/// - `4..8`: `Size: u32`
/// - `8..12`: `NumberOfIdentifiers: u32`
/// - `12..`: Array of `STORAGE_IDENTIFIER`:
///   - `+0..4`: `CodeSet: u32` (`1 = Binary`, `2 = Ascii`, `3 = Utf8`)
///   - `+4..8`: `Type: u32`
///   - `+8..10`: `IdentifierSize: u16`
///   - `+10..12`: `NextOffset: u16`
///   - `+12..16`: `Association: u32`
///   - `+16..16 + IdentifierSize`: `Identifier` bytes
pub fn patch_storage_device_id_descriptor(
    buf: &mut [u8],
    new_serial: &[u8; 20],
    seed: u64,
) -> bool {
    if buf.len() < 16 {
        return false;
    }
    let num_ids = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) as usize;
    if num_ids == 0 || num_ids > 32 {
        return false;
    }

    let mut offset = 12usize;
    let mut patched = false;
    let seed_bytes = seed.to_le_bytes();

    for _ in 0..num_ids {
        if offset + 16 > buf.len() {
            break;
        }
        let code_set = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        let id_size = u16::from_le_bytes([buf[offset + 8], buf[offset + 9]]) as usize;
        let next_offset = u16::from_le_bytes([buf[offset + 10], buf[offset + 11]]) as usize;
        let id_start = offset + 16;

        if id_size > 0 && id_start + id_size <= buf.len() {
            let id_slice = &mut buf[id_start..id_start + id_size];
            if code_set == 2 || code_set == 3 {
                // ASCII or UTF-8 VPD 0x83 identifier (often contains Vendor + Model + NSID + Serial)
                if id_slice.len() >= 20 {
                    let tail_len = if find_stornvme_nsid_suffix(id_slice).is_some() {
                        30
                    } else {
                        20
                    };
                    let tail_start = id_slice.len().saturating_sub(tail_len);
                    patch_ascii_serial_slice(&mut id_slice[tail_start..], new_serial);
                } else {
                    patch_ascii_serial_slice(id_slice, new_serial);
                }
                patched = true;
            } else if code_set == 1 && id_slice.len() >= 8 {
                // Binary EUI-64 (8 bytes) or NGUID (16 bytes):
                // Preserve first 3 bytes (IEEE OUI vendor prefix), deterministically mutate device extension bytes
                for (i, b) in id_slice[3..].iter_mut().enumerate() {
                    let mix = seed_bytes[i % 8] ^ new_serial[i % 20];
                    if mix != 0 {
                        *b ^= mix;
                    } else {
                        *b = b.wrapping_add(0x5A);
                    }
                }
                patched = true;
            }
        }

        if next_offset == 0 {
            break;
        }
        offset = offset.saturating_add(next_offset);
    }

    patched
}

/// 1C. Patches `STORAGE_PROTOCOL_DATA_DESCRIPTOR` (`IOCTL_STORAGE_QUERY_PROPERTY`, `PropertyId = 49 / 50`).
///
/// Layout (`ntddstor.h`):
/// - `0..4`: `Version: u32`
/// - `4..8`: `Size: u32`
/// - `8..48`: `STORAGE_PROTOCOL_SPECIFIC_DATA`:
///   - `8..12`: `ProtocolType: u32` (`2 = ATA`, `3 = NVMe`)
///   - `12..16`: `DataType: u32` (`1 = Identify`)
///   - `16..20`: `ProtocolDataRequestValue: u32` (`1 = NVME_IDENTIFY_CNS_CONTROLLER`)
///   - `24..28`: `ProtocolDataOffset: u32` (offset from start of `STORAGE_PROTOCOL_SPECIFIC_DATA`, i.e. byte 8!)
///   - `28..32`: `ProtocolDataLength: u32`
pub fn patch_storage_protocol_descriptor(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if buf.len() < 48 {
        return false;
    }
    let protocol_type = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let data_type = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);
    let request_val = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]);
    let data_offset = u32::from_le_bytes([buf[24], buf[25], buf[26], buf[27]]) as usize;
    let data_len = u32::from_le_bytes([buf[28], buf[29], buf[30], buf[31]]) as usize;

    if data_type != PROTOCOL_DATA_TYPE_IDENTIFY {
        return false;
    }

    // `ProtocolDataOffset` is measured from the start of `STORAGE_PROTOCOL_SPECIFIC_DATA` (offset 8)
    let abs_offset = 8usize.saturating_add(data_offset);
    if abs_offset >= buf.len() {
        return false;
    }

    if protocol_type == PROTOCOL_TYPE_NVME {
        // NVMe Identify Controller (`CNS == 1`, or 4096-byte buffer with ASCII SN at 4..24)
        if abs_offset + 64 <= buf.len() && data_len >= 64 {
            let ident = &mut buf[abs_offset..];
            if request_val == 1 || is_printable_ascii(&ident[4..24]) {
                ident[4..24].copy_from_slice(new_serial);
                return true;
            }
        }
    } else if protocol_type == PROTOCOL_TYPE_ATA
        && abs_offset + 40 <= buf.len()
        && data_len >= 40
    {
        write_ata_word_swapped(&mut buf[abs_offset + 20..abs_offset + 40], new_serial);
        return true;
    }

    false
}

/// 1D. Patches `STORAGE_ADAPTER_SERIAL_NUMBER` (`IOCTL_STORAGE_QUERY_PROPERTY`, `PropertyId = 56`).
///
/// Layout (`ntddstor.h`):
/// - `0..4`: `Version: u32`
/// - `4..8`: `Size: u32`
/// - `8..264`: `WCHAR SerialNumber[128]` (UTF-16LE null-terminated string)
pub fn patch_storage_adapter_serial_number(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if buf.len() < 48 {
        return false;
    }
    let wchar_bytes = &mut buf[8..];
    let max_chars = core::cmp::min(wchar_bytes.len() / 2, 127);
    let mut char_len = 0usize;
    while char_len < max_chars {
        let lo = wchar_bytes[char_len * 2];
        let hi = wchar_bytes[char_len * 2 + 1];
        if lo == 0 && hi == 0 {
            break;
        }
        char_len += 1;
    }
    if char_len == 0 {
        return false;
    }

    let write_len = core::cmp::min(char_len, new_serial.len());
    for i in 0..write_len {
        wchar_bytes[i * 2] = new_serial[i];
        wchar_bytes[i * 2 + 1] = 0;
    }
    true
}

/// 2. Patches `STORAGE_PROTOCOL_COMMAND` (`IOCTL_STORAGE_PROTOCOL_COMMAND`).
///
/// Layout (`ntddstor.h`):
/// - `8..12`: `ProtocolType: u32` (`3 = ProtocolTypeNvme`)
/// - `32..36`: `DataFromDeviceTransferLength: u32`
/// - `48..52`: `DataFromDeviceBufferOffset: u32`
pub fn patch_storage_protocol_command(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if buf.len() < 56 {
        return false;
    }
    let protocol_type = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    let from_len = u32::from_le_bytes([buf[32], buf[33], buf[34], buf[35]]) as usize;
    let from_offset = u32::from_le_bytes([buf[48], buf[49], buf[50], buf[51]]) as usize;

    if protocol_type == PROTOCOL_TYPE_NVME
        && from_len >= 64
        && from_offset >= 52
        && from_offset + 64 <= buf.len()
    {
        let data = &mut buf[from_offset..];
        if is_printable_ascii(&data[4..24]) && is_printable_ascii(&data[24..64]) {
            data[4..24].copy_from_slice(new_serial);
            return true;
        }
    }
    false
}

/// 3. Patches `SENDCMDOUTPARAMS` (`SMART_RCV_DRIVE_DATA`).
///
/// Layout (`ntdddisk.h`, `#pragma pack(1)`):
/// - `0..4`: `cBufferSize: u32`
/// - `4..16`: `DriverStatus: DRIVERSTATUS` (12 bytes)
/// - `16..528`: `bBuffer: [u8; 512]` (ATA `IDENTIFY DEVICE` sector; serial at `bBuffer[20..40]`)
pub fn patch_smart_rcv_drive_data(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    // Offset 16 + 40 = 56 bytes minimum
    if buf.len() < 56 {
        return false;
    }
    write_ata_word_swapped(&mut buf[36..56], new_serial);
    true
}

/// 4. Patches `ATA_PASS_THROUGH_EX` (`IOCTL_ATA_PASS_THROUGH`, 64-bit layout).
///
/// Layout (`ntddscsi.h` on x64):
/// - `0..2`: `Length: u16` (48 on x64)
/// - `16..20`: `DataTransferLength: u32`
/// - `24..32`: `DataBufferOffset: usize` (`u64` on x64)
pub fn patch_ata_pass_through(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if buf.len() < 48 {
        return false;
    }
    let transfer_len = u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]) as usize;
    let data_offset = u64::from_le_bytes([
        buf[24], buf[25], buf[26], buf[27], buf[28], buf[29], buf[30], buf[31],
    ]) as usize;

    if transfer_len >= 512 && data_offset >= 40 && data_offset + 40 <= buf.len() {
        let sector = &mut buf[data_offset..];
        if is_printable_ascii(&sector[20..40]) {
            write_ata_word_swapped(&mut sector[20..40], new_serial);
            return true;
        }
    }
    false
}

/// 5. Patches `SCSI_PASS_THROUGH` (`IOCTL_SCSI_PASS_THROUGH`, 64-bit layout) when CDB is `INQUIRY (0x12)`.
pub fn patch_scsi_pass_through(buf: &mut [u8], new_serial: &[u8; 20]) -> bool {
    // On x64, `SCSI_PASS_THROUGH` has `DataBufferOffset` at 32..40 and `Cdb[16]` at 44..60
    if buf.len() < 64 {
        return false;
    }
    let data_offset = u64::from_le_bytes([
        buf[32], buf[33], buf[34], buf[35], buf[36], buf[37], buf[38], buf[39],
    ]) as usize;
    if data_offset < 56 || data_offset + 4 > buf.len() {
        return false;
    }
    patch_scsi_vpd_inquiry(&mut buf[data_offset..], new_serial)
}

/// Patches a raw SCSI `INQUIRY` response buffer (VPD Page `0x80` Unit Serial Number or Standard Inquiry).
pub fn patch_scsi_vpd_inquiry(inq: &mut [u8], new_serial: &[u8; 20]) -> bool {
    if inq.len() < 8 {
        return false;
    }
    // VPD Page 0x80 (`Unit Serial Number`): byte 1 == 0x80, byte 3 == page_length
    if inq[1] == 0x80 {
        let page_len = inq[3] as usize;
        if page_len > 0 && 4 + page_len <= inq.len() {
            return patch_ascii_serial_slice(&mut inq[4..4 + page_len], new_serial);
        }
    }
    // Standard Inquiry (`byte 1 == 0x00`, vendor-specific serial at 36..56 if printable)
    if inq[1] == 0x00 && inq.len() >= 56 && is_printable_ascii(&inq[36..56]) {
        return patch_ascii_serial_slice(&mut inq[36..56], new_serial);
    }
    false
}

/// Scans `buf` for any ASCII or UTF-16LE occurrences of the original disk serial prefix
/// (`orig_serial` trimmed of trailing spaces) and replaces them in-place with `new_serial`.
///
/// This catches `StorageDeviceUniqueIdProperty (3)`, `\Device\Spaceport` (`Get-PhysicalDisk`),
/// and custom vendor IOCTLs that embed the factory serial inside composite descriptors.
pub fn patch_buffer_occurrences_ascii_and_utf16(
    buf: &mut [u8],
    orig_serial: &[u8; 20],
    new_serial: &[u8; 20],
) -> bool {
    // Compute trimmed non-space length of `orig_serial`
    let mut orig_len = orig_serial.len();
    while orig_len > 0 && (orig_serial[orig_len - 1] == b' ' || orig_serial[orig_len - 1] == 0) {
        orig_len -= 1;
    }
    if orig_len < 6 || buf.len() < orig_len {
        return false;
    }

    let orig_prefix = &orig_serial[..orig_len];
    let mut patched = false;

    // 1. Scan for ASCII occurrences of `orig_prefix`
    let mut i = 0usize;
    while i + orig_len <= buf.len() {
        if &buf[i..i + orig_len] == orig_prefix {
            for k in 0..orig_len {
                let rep = new_serial[k];
                buf[i + k] = if rep != b' ' && rep != 0 { rep } else { b'0' };
            }
            patched = true;
            i += orig_len;
        } else {
            i += 1;
        }
    }

    // 2. Scan for UTF-16LE occurrences of `orig_prefix` (`2 * orig_len` bytes)
    let utf16_len = orig_len * 2;
    if buf.len() >= utf16_len {
        let mut j = 0usize;
        while j + utf16_len <= buf.len() {
            let mut matches = true;
            for k in 0..orig_len {
                if buf[j + k * 2] != orig_prefix[k] || buf[j + k * 2 + 1] != 0 {
                    matches = false;
                    break;
                }
            }
            if matches {
                for k in 0..orig_len {
                    let rep = new_serial[k];
                    buf[j + k * 2] = if rep != b' ' && rep != 0 { rep } else { b'0' };
                    buf[j + k * 2 + 1] = 0;
                }
                patched = true;
                j += utf16_len;
            } else {
                j += 1;
            }
        }
    }

    patched
}

#[inline]
fn write_ata_word_swapped(dst_20: &mut [u8], new_serial: &[u8; 20]) {
    for i in (0..20).step_by(2) {
        dst_20[i] = new_serial[i + 1];
        dst_20[i + 1] = new_serial[i];
    }
}

#[inline]
fn is_printable_ascii(bytes: &[u8]) -> bool {
    let mut non_space = 0usize;
    for &b in bytes {
        if !(0x20..=0x7E).contains(&b) {
            return false;
        }
        if b != b' ' {
            non_space += 1;
        }
    }
    non_space >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_patch_storage_device_descriptor_stornvme_format() {
        // Build a synthetic STORAGE_DEVICE_DESCRIPTOR with `CDA7N706410107T15   _00000001.\0` at offset 40
        let mut buf = [0u8; 128];
        buf[0..4].copy_from_slice(&40u32.to_le_bytes()); // Version
        buf[4..8].copy_from_slice(&128u32.to_le_bytes()); // Size
        buf[24..28].copy_from_slice(&40u32.to_le_bytes()); // SerialNumberOffset = 40
        let orig = b"CDA7N706410107T15   _00000001.\0";
        buf[40..40 + orig.len()].copy_from_slice(orig);

        let new_serial = b"CA54N299714909Q56   ";
        let req = [0u8, 0, 0, 0, 0, 0, 0, 0]; // StorageDeviceProperty (0), PropertyStandardQuery (0)
        let tag = classify_ioctl_request(IOCTL_STORAGE_QUERY_PROPERTY, &req);
        assert_eq!(tag, TAG_STORAGE_DEVICE_DESCRIPTOR);

        assert!(patch_ioctl_output_by_tag(tag, &mut buf, new_serial, 0x1234));
        assert_eq!(&buf[40..70], b"CA54N299714909Q56   _00000001.");
        assert_eq!(buf[70], 0);
    }

    #[test]
    fn test_patch_storage_protocol_descriptor_nvme_identify() {
        // 48-byte header + 4096-byte NVMe Identify Controller payload
        let mut buf = [0u8; 48 + 4096];
        buf[0..4].copy_from_slice(&48u32.to_le_bytes());
        buf[4..8].copy_from_slice(&48u32.to_le_bytes());
        buf[8..12].copy_from_slice(&PROTOCOL_TYPE_NVME.to_le_bytes());
        buf[12..16].copy_from_slice(&PROTOCOL_DATA_TYPE_IDENTIFY.to_le_bytes());
        buf[16..20].copy_from_slice(&1u32.to_le_bytes()); // CNS = 1 (Controller)
        buf[24..28].copy_from_slice(&40u32.to_le_bytes()); // Offset from byte 8 -> 8 + 40 = 48
        buf[28..32].copy_from_slice(&4096u32.to_le_bytes());

        buf[48 + 4..48 + 24].copy_from_slice(b"CDA7N706410107T15   ");
        buf[48 + 24..48 + 64].copy_from_slice(b"HFM001TD3JX013N                         ");

        let new_serial = b"CA54N299714909Q56   ";
        assert!(patch_storage_protocol_descriptor(&mut buf, new_serial));
        assert_eq!(&buf[48 + 4..48 + 24], new_serial);
        // Model number must remain untouched
        assert_eq!(
            &buf[48 + 24..48 + 64],
            b"HFM001TD3JX013N                         "
        );
    }

    #[test]
    fn test_patch_smart_rcv_drive_data_word_swapped() {
        let mut buf = [0u8; 528];
        buf[0..4].copy_from_slice(&512u32.to_le_bytes());
        let new_serial = b"S4EVNF0M123456A     ";
        assert!(patch_smart_rcv_drive_data(&mut buf, new_serial));
        // Pair-swapped check: "S4" -> "4S"
        assert_eq!(buf[36], b'4');
        assert_eq!(buf[37], b'S');
        assert_eq!(buf[38], b'V');
        assert_eq!(buf[39], b'E');
    }

    #[test]
    fn test_patch_storage_adapter_serial_utf16() {
        let mut buf = [0u8; 264];
        buf[0..4].copy_from_slice(&264u32.to_le_bytes());
        buf[4..8].copy_from_slice(&264u32.to_le_bytes());
        for (i, &b) in b"CDA7N706410107T15   ".iter().enumerate() {
            buf[8 + i * 2] = b;
            buf[8 + i * 2 + 1] = 0;
        }
        let new_serial = b"CA54N299714909Q56   ";
        assert!(patch_storage_adapter_serial_number(&mut buf, new_serial));
        for (i, &b) in new_serial.iter().enumerate() {
            assert_eq!(buf[8 + i * 2], b);
            assert_eq!(buf[8 + i * 2 + 1], 0);
        }
    }

    #[test]
    fn test_patch_vpd83_t10_unique_id_preserves_nsid() {
        // Exact 68-byte VPD 0x83 string observed on SK Hynix NVMe:
        // "1C5CHFM001TD3JX013N                         0001CDA7N706410107T15   "
        let vpd_str = b"1C5CHFM001TD3JX013N                         0001CDA7N706410107T15   ";
        let mut buf = [0u8; 12 + 16 + 68];
        let total_size = buf.len() as u32;
        buf[0..4].copy_from_slice(&1u32.to_le_bytes()); // Version
        buf[4..8].copy_from_slice(&total_size.to_le_bytes()); // Size
        buf[8..12].copy_from_slice(&1u32.to_le_bytes()); // NumberOfIdentifiers = 1
        // STORAGE_IDENTIFIER at offset 12:
        buf[12..16].copy_from_slice(&2u32.to_le_bytes()); // CodeSet = 2 (Ascii)
        buf[20..22].copy_from_slice(&68u16.to_le_bytes()); // IdentifierSize = 68
        buf[28..28 + 68].copy_from_slice(vpd_str);

        let new_serial = b"YF0W826954803M86Y   ";
        assert!(patch_storage_device_id_descriptor(&mut buf, new_serial, 0x9999));
        assert_eq!(
            &buf[28..28 + 68],
            b"1C5CHFM001TD3JX013N                         0001YF0W826954803M86Y   "
        );
    }

    #[test]
    fn test_patch_buffer_occurrences_ascii_and_utf16() {
        let orig = b"CDA7N706410107T15   ";
        let spoof = b"YF0W826954803M86Y   ";
        let mut buf = [0u8; 128];
        // Place ASCII at offset 8
        buf[8..38].copy_from_slice(b"CDA7N706410107T15   _00000001.");
        // Place UTF-16LE at offset 48
        for (i, &b) in b"CDA7N706410107T15".iter().enumerate() {
            buf[48 + i * 2] = b;
            buf[48 + i * 2 + 1] = 0;
        }

        assert!(patch_buffer_occurrences_ascii_and_utf16(&mut buf, orig, spoof));
        assert_eq!(&buf[8..38], b"YF0W826954803M86Y   _00000001.");
        for (i, &b) in b"YF0W826954803M86Y".iter().enumerate() {
            assert_eq!(buf[48 + i * 2], b);
            assert_eq!(buf[48 + i * 2 + 1], 0);
        }
    }
}
