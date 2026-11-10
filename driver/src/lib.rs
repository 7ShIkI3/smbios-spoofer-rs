//! `smbios-disk-filter.sys` — Windows Kernel WDM Storage Filter Driver (`Ring 0`, `#![no_std]`).
//!
//! Synchronizes the NVMe/SATA/SCSI disk serial number generated in UEFI (`SmbiosSpoof` v9)
//! across the `ExitBootServices()` boundary into Windows (`stornvme.sys` / `disk.sys` / WMI).
//!
//! ### Architecture
//! 1. **Zero-WDK Compilation**: Uses Rust's `#[link(name = "ntoskrnl.exe", kind = "raw-dylib")]`
//!    to synthesize the PE import table directly at compile time.
//! 2. **Multi-Source Profile Bridge (`v9`)**:
//!    - Source A: UEFI Runtime Variable `SmbiosSpoof` (`8BE4DF61-93CA-11D2-AA0D-00E098032B8C`)
//!      via `ExGetFirmwareEnvironmentVariable`.
//!    - Source B: Direct kernel read of the EFI System Partition file
//!      `\Device\HarddiskVolume1..8\EFI\SmbiosSpoofer\uuid.dat` via `ZwOpenFile` + `ZwReadFile`
//!      (works even when the ESP has no drive letter mounted in Windows!).
//! 3. **Safe Standalone Stack Attachment**:
//!    - Attaches a Filter Device Object (`FiDO`) to `\Device\Harddisk0\DR0` .. `\Device\Harddisk15\DR15`
//!      via `IoAttachDeviceToDeviceStackSafe`.
//!    - All `IRP_MJ_READ`, `IRP_MJ_WRITE`, `IRP_MJ_PNP`, and `IRP_MJ_POWER` IRPs pass straight
//!      through via `IoSkipCurrentIrpStackLocation` with zero overhead.
//!    - Intercepts `IRP_MJ_DEVICE_CONTROL` completion for `IOCTL_STORAGE_QUERY_PROPERTY`,
//!      `IOCTL_STORAGE_PROTOCOL_COMMAND`, `SMART_RCV_DRIVE_DATA`, `IOCTL_ATA_PASS_THROUGH`,
//!      and `IOCTL_SCSI_PASS_THROUGH`.

#![no_std]
#![no_main]
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::ffi::c_void;
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicU64, Ordering};

#[path = "../../src/win_disk_patch.rs"]
pub mod win_disk_patch;

// ─── Global Spoofed Disk Serial State (Loaded in DriverEntry) ──────────────

static mut SPOOFED_DISK_SERIAL: [u8; 20] = *b"CA54N299714909Q56   ";
static SPOOFED_DISK_SEED: AtomicU64 = AtomicU64::new(0x534D_4249_2026_0009);

// Force MSVC link.exe to emit a non-empty `.reloc` Base Relocation Directory for MmLoadSystemImage
#[used]
static RELOC_ANCHOR: unsafe extern "system" fn(*mut DRIVER_OBJECT, *const UNICODE_STRING) -> NTSTATUS =
    DriverEntry;

// ─── WDM Constants & NTSTATUS ──────────────────────────────────────────────

pub type NTSTATUS = i32;
pub const STATUS_SUCCESS: NTSTATUS = 0;
pub const STATUS_UNSUCCESSFUL: NTSTATUS = -1073741823; // 0xC0000001

pub const IRP_MJ_MAXIMUM_FUNCTION: usize = 27;
pub const IRP_MJ_DEVICE_CONTROL: usize = 0x0E;
pub const IRP_MJ_INTERNAL_DEVICE_CONTROL: usize = 0x0F; // IRP_MJ_SCSI
pub const IRP_MJ_POWER: usize = 0x16;

pub const DO_BUFFERED_IO: u32 = 0x0000_0004;
pub const DO_DIRECT_IO: u32 = 0x0000_0010;
pub const DO_DEVICE_INITIALIZING: u32 = 0x0000_0080;
pub const DO_POWER_PAGABLE: u32 = 0x0000_2000;

pub const FILE_READ_ATTRIBUTES: u32 = 0x0000_0080;
pub const FILE_GENERIC_READ: u32 = 0x0012_0089;
pub const FILE_SHARE_READ: u32 = 0x0000_0001;
pub const FILE_SHARE_WRITE: u32 = 0x0000_0002;
pub const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;

pub const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
pub const OBJ_KERNEL_HANDLE: u32 = 0x0000_0200;

pub const SL_PENDING_RETURNED: u8 = 0x01;
pub const SL_INVOKE_ON_CANCEL: u8 = 0x20;
pub const SL_INVOKE_ON_SUCCESS: u8 = 0x40;
pub const SL_INVOKE_ON_ERROR: u8 = 0x80;

pub const SPOOF_CONFIG_MAGIC: u32 = 0x534D_4249; // "SMBI"
pub const SPOOF_CONFIG_V9_SIZE: usize = 1710;
pub const SPOOF_CONFIG_DISK_SERIAL_OFFSET: usize = 1574; // [u16; 64] = 128 bytes
pub const SPOOF_CONFIG_DISK_SEED_OFFSET: usize = 1702; // u32 = 4 bytes

// ─── Binary-Verified x64 WDM Kernel Structures ─────────────────────────────

#[repr(C)]
#[derive(Copy, Clone)]
pub struct GUID {
    pub Data1: u32,
    pub Data2: u16,
    pub Data3: u16,
    pub Data4: [u8; 8],
}

/// `8BE4DF61-93CA-11D2-AA0D-00E098032B8C`
const NVRAM_VENDOR_GUID: GUID = GUID {
    Data1: 0x8BE4_DF61,
    Data2: 0x93CA,
    Data3: 0x11D2,
    Data4: [0xAA, 0x0D, 0x00, 0xE0, 0x98, 0x03, 0x2B, 0x8C],
};

#[repr(C)]
#[derive(Copy, Clone)]
pub struct UNICODE_STRING {
    pub Length: u16,
    pub MaximumLength: u16,
    pub _pad: u32,
    pub Buffer: *const u16,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct LIST_ENTRY {
    pub Flink: *mut LIST_ENTRY,
    pub Blink: *mut LIST_ENTRY,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IO_STATUS_BLOCK {
    pub Status: NTSTATUS,
    pub _pad: u32,
    pub Information: usize,
}

#[repr(C)]
pub struct OBJECT_ATTRIBUTES {
    pub Length: u32,
    pub _pad0: u32,
    pub RootDirectory: *mut c_void,
    pub ObjectName: *const UNICODE_STRING,
    pub Attributes: u32,
    pub _pad1: u32,
    pub SecurityDescriptor: *mut c_void,
    pub SecurityQualityOfService: *mut c_void,
}

pub type PDRIVER_DISPATCH =
    Option<unsafe extern "system" fn(DeviceObject: *mut DEVICE_OBJECT, Irp: *mut IRP) -> NTSTATUS>;
pub type PDRIVER_UNLOAD = Option<unsafe extern "system" fn(DriverObject: *mut DRIVER_OBJECT)>;
pub type PIO_COMPLETION_ROUTINE = Option<
    unsafe extern "system" fn(
        DeviceObject: *mut DEVICE_OBJECT,
        Irp: *mut IRP,
        Context: *mut c_void,
    ) -> NTSTATUS,
>;

#[repr(C)]
pub struct DRIVER_OBJECT {
    pub Type: i16,
    pub Size: i16,
    pub _pad0: u32,
    pub DeviceObject: *mut DEVICE_OBJECT,
    pub Flags: u32,
    pub _pad1: u32,
    pub DriverStart: *mut c_void,
    pub DriverSize: u32,
    pub _pad2: u32,
    pub DriverSection: *mut c_void,
    pub DriverExtension: *mut c_void,
    pub DriverName: UNICODE_STRING,
    pub HardwareDatabase: *mut UNICODE_STRING,
    pub FastIoDispatch: *mut c_void,
    pub DriverInit: *mut c_void,
    pub DriverStartIo: *mut c_void,
    pub DriverUnload: PDRIVER_UNLOAD,
    pub MajorFunction: [PDRIVER_DISPATCH; IRP_MJ_MAXIMUM_FUNCTION + 1],
}

#[repr(C)]
pub struct DEVICE_OBJECT {
    pub Type: i16,
    pub Size: u16,
    pub ReferenceCount: i32,
    pub DriverObject: *mut DRIVER_OBJECT,
    pub NextDevice: *mut DEVICE_OBJECT,
    pub AttachedDevice: *mut DEVICE_OBJECT,
    pub CurrentIrp: *mut IRP,
    pub Timer: *mut c_void,
    pub Flags: u32,
    pub Characteristics: u32,
    pub Vpb: *mut c_void,
    pub DeviceExtension: *mut c_void,
    pub DeviceType: u32,
    pub StackSize: i8,
    pub _pad: [u8; 3],
}

/// `_IO_STACK_LOCATION` on 64-bit Windows (`size = 0x48 = 72` bytes).
/// Includes `POINTER_ALIGNMENT` (`__declspec(align(8))`) on `InputBufferLength` and `IoControlCode`.
#[repr(C)]
pub struct IO_STACK_LOCATION {
    pub MajorFunction: u8,
    pub MinorFunction: u8,
    pub Flags: u8,
    pub Control: u8,
    pub _pad0: u32,
    // Parameters.DeviceIoControl (32 bytes: 0x08..0x28):
    pub OutputBufferLength: u32,
    pub _pad1: u32,
    pub InputBufferLength: u32,
    pub _pad2: u32,
    pub IoControlCode: u32,
    pub _pad3: u32,
    pub Type3InputBuffer: *mut c_void,
    // Common tail fields (0x28..0x48):
    pub DeviceObject: *mut DEVICE_OBJECT,
    pub FileObject: *mut c_void,
    pub CompletionRoutine: PIO_COMPLETION_ROUTINE,
    pub Context: *mut c_void,
}

/// `_IRP` on 64-bit Windows (`size = 0xD0 = 208` bytes).
#[repr(C)]
pub struct IRP {
    pub Type: i16,
    pub Size: u16,
    pub _pad0: u32,
    pub MdlAddress: *mut c_void,
    pub Flags: u32,
    pub _pad1: u32,
    pub SystemBuffer: *mut c_void, // +0x18: AssociatedIrp.SystemBuffer
    pub ThreadListEntry: LIST_ENTRY,
    pub IoStatus: IO_STATUS_BLOCK, // +0x30: Status (+0x30), Information (+0x38)
    pub RequestorMode: i8,
    pub PendingReturned: u8, // +0x41
    pub StackCount: i8,      // +0x42
    pub CurrentLocation: i8, // +0x43
    pub Cancel: u8,
    pub CancelIrql: u8,
    pub ApcEnvironment: i8,
    pub AllocationFlags: u8,
    pub UserIosb: *mut IO_STATUS_BLOCK,
    pub UserEvent: *mut c_void,
    pub Overlay: [u64; 2],
    pub CancelRoutine: *mut c_void,
    pub UserBuffer: *mut c_void, // +0x70
    // Tail.Overlay (+0x78..+0xD0):
    pub DriverContext: [*mut c_void; 4], // +0x78..+0x98
    pub Thread: *mut c_void,             // +0x98
    pub AuxiliaryBuffer: *mut u8,        // +0xA0
    pub ListEntry: LIST_ENTRY,           // +0xA8..+0xB8
    pub CurrentStackLocation: *mut IO_STACK_LOCATION, // +0xB8
    pub OriginalFileObject: *mut c_void, // +0xC0
    pub IrpExtension: *mut c_void,       // +0xC8
}

// Compile-time ABI offset assertions for x64 Windows Kernel structures
const _: () = {
    assert!(core::mem::size_of::<DRIVER_OBJECT>() == 0x150);
    assert!(core::mem::offset_of!(DRIVER_OBJECT, MajorFunction) == 0x70);
    assert!(core::mem::offset_of!(DEVICE_OBJECT, Flags) == 0x30);
    assert!(core::mem::offset_of!(DEVICE_OBJECT, DeviceExtension) == 0x40);
    assert!(core::mem::offset_of!(DEVICE_OBJECT, StackSize) == 0x4C);
    assert!(core::mem::size_of::<IO_STACK_LOCATION>() == 0x48);
    assert!(core::mem::offset_of!(IO_STACK_LOCATION, OutputBufferLength) == 0x08);
    assert!(core::mem::offset_of!(IO_STACK_LOCATION, InputBufferLength) == 0x10);
    assert!(core::mem::offset_of!(IO_STACK_LOCATION, IoControlCode) == 0x18);
    assert!(core::mem::offset_of!(IO_STACK_LOCATION, CompletionRoutine) == 0x38);
    assert!(core::mem::offset_of!(IO_STACK_LOCATION, Context) == 0x40);
    assert!(core::mem::size_of::<IRP>() == 0xD0);
    assert!(core::mem::offset_of!(IRP, SystemBuffer) == 0x18);
    assert!(core::mem::offset_of!(IRP, IoStatus) == 0x30);
    assert!(core::mem::offset_of!(IRP, PendingReturned) == 0x41);
    assert!(core::mem::offset_of!(IRP, CurrentLocation) == 0x43);
    assert!(core::mem::offset_of!(IRP, CurrentStackLocation) == 0xB8);
};

#[repr(C)]
struct FilterExtension {
    lower_device: *mut DEVICE_OBJECT,
    disk_index: u32,
}

// ─── Direct `ntoskrnl.exe` Raw-Dylib Imports ───────────────────────────────

unsafe extern "system" {
    fn IoCreateDevice(
        DriverObject: *mut DRIVER_OBJECT,
        DeviceExtensionSize: u32,
        DeviceName: *const UNICODE_STRING,
        DeviceType: u32,
        DeviceCharacteristics: u32,
        Exclusive: u8,
        DeviceObject: *mut *mut DEVICE_OBJECT,
    ) -> NTSTATUS;

    fn IoDeleteDevice(DeviceObject: *mut DEVICE_OBJECT);

    fn IoGetDeviceObjectPointer(
        ObjectName: *const UNICODE_STRING,
        DesiredAccess: u32,
        FileObject: *mut *mut c_void,
        DeviceObject: *mut *mut DEVICE_OBJECT,
    ) -> NTSTATUS;

    fn IoAttachDeviceToDeviceStackSafe(
        SourceDevice: *mut DEVICE_OBJECT,
        TargetDevice: *mut DEVICE_OBJECT,
        AttachedToDeviceObject: *mut *mut DEVICE_OBJECT,
    ) -> NTSTATUS;

    fn IofCallDriver(DeviceObject: *mut DEVICE_OBJECT, Irp: *mut IRP) -> NTSTATUS;
    fn PoCallDriver(DeviceObject: *mut DEVICE_OBJECT, Irp: *mut IRP) -> NTSTATUS;
    fn PoStartNextPowerIrp(Irp: *mut IRP);
    fn ObfDereferenceObject(Object: *mut c_void);

    fn ExGetFirmwareEnvironmentVariable(
        VariableName: *const UNICODE_STRING,
        VendorGuid: *const GUID,
        Value: *mut c_void,
        ValueLength: *mut u32,
        Attributes: *mut u32,
    ) -> NTSTATUS;

    fn ZwOpenFile(
        FileHandle: *mut *mut c_void,
        DesiredAccess: u32,
        ObjectAttributes: *const OBJECT_ATTRIBUTES,
        IoStatusBlock: *mut IO_STATUS_BLOCK,
        ShareAccess: u32,
        OpenOptions: u32,
    ) -> NTSTATUS;

    fn ZwReadFile(
        FileHandle: *mut c_void,
        Event: *mut c_void,
        ApcRoutine: *mut c_void,
        ApcContext: *mut c_void,
        IoStatusBlock: *mut IO_STATUS_BLOCK,
        Buffer: *mut c_void,
        Length: u32,
        ByteOffset: *const i64,
        Key: *const u32,
    ) -> NTSTATUS;

    fn ZwClose(Handle: *mut c_void) -> NTSTATUS;
}

unsafe extern "C" {
    pub fn memset(dest: *mut c_void, c: i32, count: usize) -> *mut c_void;
    pub fn memcpy(dest: *mut c_void, src: *const c_void, count: usize) -> *mut c_void;
    pub fn memmove(dest: *mut c_void, src: *const c_void, count: usize) -> *mut c_void;
    pub fn memcmp(buf1: *const c_void, buf2: *const c_void, count: usize) -> i32;
}

// ─── Inline WDM Stack Helpers (`wdm.h` equivalents) ────────────────────────

#[inline(always)]
unsafe fn io_get_current_irp_stack_location(irp: *mut IRP) -> *mut IO_STACK_LOCATION {
    unsafe { (*irp).CurrentStackLocation }
}

#[inline(always)]
unsafe fn io_get_next_irp_stack_location(irp: *mut IRP) -> *mut IO_STACK_LOCATION {
    unsafe { (*irp).CurrentStackLocation.sub(1) }
}

#[inline(always)]
unsafe fn io_skip_current_irp_stack_location(irp: *mut IRP) {
    unsafe {
        (*irp).CurrentLocation += 1;
        (*irp).CurrentStackLocation = (*irp).CurrentStackLocation.add(1);
    }
}

#[inline(always)]
unsafe fn io_copy_current_irp_stack_location_to_next(irp: *mut IRP) {
    unsafe {
        let irp_sp = io_get_current_irp_stack_location(irp);
        let next_sp = io_get_next_irp_stack_location(irp);
        ptr::copy_nonoverlapping(
            irp_sp as *const u8,
            next_sp as *mut u8,
            core::mem::offset_of!(IO_STACK_LOCATION, CompletionRoutine),
        );
        (*next_sp).Control = 0;
    }
}

#[inline(always)]
unsafe fn io_set_completion_routine(
    irp: *mut IRP,
    routine: PIO_COMPLETION_ROUTINE,
    context: *mut c_void,
) {
    unsafe {
        let next_sp = io_get_next_irp_stack_location(irp);
        (*next_sp).CompletionRoutine = routine;
        (*next_sp).Context = context;
        (*next_sp).Control = SL_INVOKE_ON_SUCCESS | SL_INVOKE_ON_ERROR | SL_INVOKE_ON_CANCEL;
    }
}

// ─── Profile Loading (UEFI NVRAM + ESP `\EFI\SmbiosSpoofer\uuid.dat`) ──────

/// Parses a raw `SpoofConfig` (`v9`, 1710 bytes) buffer and extracts `disk_serial` + `disk_serial_seed`.
fn parse_v9_config_bytes(raw: &[u8]) -> bool {
    if raw.len() < SPOOF_CONFIG_V9_SIZE {
        return false;
    }
    let magic = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    let version = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
    if magic != SPOOF_CONFIG_MAGIC || version < 9 {
        return false;
    }

    let mut serial_20 = [b' '; 20];
    let mut non_space = 0usize;
    for (i, slot) in serial_20.iter_mut().enumerate() {
        let off = SPOOF_CONFIG_DISK_SERIAL_OFFSET + i * 2;
        let ch = u16::from_le_bytes([raw[off], raw[off + 1]]);
        if ch == 0 {
            break;
        }
        if (0x20..=0x7E).contains(&ch) {
            *slot = ch as u8;
            if ch as u8 != b' ' {
                non_space += 1;
            }
        }
    }

    if non_space < 4 {
        return false;
    }

    let seed = u32::from_le_bytes([
        raw[SPOOF_CONFIG_DISK_SEED_OFFSET],
        raw[SPOOF_CONFIG_DISK_SEED_OFFSET + 1],
        raw[SPOOF_CONFIG_DISK_SEED_OFFSET + 2],
        raw[SPOOF_CONFIG_DISK_SEED_OFFSET + 3],
    ]) as u64;

    unsafe {
        SPOOFED_DISK_SERIAL = serial_20;
    }
    SPOOFED_DISK_SEED.store(if seed != 0 { seed } else { 0x534D_4249 }, Ordering::Relaxed);
    true
}

/// Attempts to load `SpoofConfig` (`v9`) from UEFI NVRAM (`SmbiosSpoof`) or directly from
/// `\Device\HarddiskVolume1..8\EFI\SmbiosSpoofer\uuid.dat`.
unsafe fn load_spoof_config_from_uefi_or_esp() {
    let mut raw_buf = [0u8; 1728];

    // 1. Try UEFI Runtime Variable `SmbiosSpoof`
    // UTF-16LE for "SmbiosSpoof\0"
    const VAR_NAME_UTF16: [u16; 12] = [
        b'S' as u16,
        b'm' as u16,
        b'b' as u16,
        b'i' as u16,
        b'o' as u16,
        b's' as u16,
        b'S' as u16,
        b'p' as u16,
        b'o' as u16,
        b'o' as u16,
        b'f' as u16,
        0,
    ];
    let var_ustr = UNICODE_STRING {
        Length: 22,
        MaximumLength: 24,
        _pad: 0,
        Buffer: VAR_NAME_UTF16.as_ptr(),
    };
    let mut val_len = raw_buf.len() as u32;
    let status = unsafe {
        ExGetFirmwareEnvironmentVariable(
            &var_ustr,
            &NVRAM_VENDOR_GUID,
            raw_buf.as_mut_ptr() as *mut c_void,
            &mut val_len,
            ptr::null_mut(),
        )
    };
    if status >= 0 && parse_v9_config_bytes(&raw_buf[..val_len as usize]) {
        return;
    }

    // 2. Fallback: Try reading `\Device\HarddiskVolume1..8\EFI\SmbiosSpoofer\uuid.dat`
    // Template: `\Device\HarddiskVolumeX\EFI\SmbiosSpoofer\uuid.dat` (50 chars)
    let mut path_utf16: [u16; 52] = [
        b'\\' as u16, b'D' as u16, b'e' as u16, b'v' as u16, b'i' as u16, b'c' as u16,
        b'e' as u16, b'\\' as u16, b'H' as u16, b'a' as u16, b'r' as u16, b'd' as u16,
        b'd' as u16, b'i' as u16, b's' as u16, b'k' as u16, b'V' as u16, b'o' as u16,
        b'l' as u16, b'u' as u16, b'm' as u16, b'e' as u16,
        b'1' as u16, // index 22: Volume digit ('1'..'8')
        b'\\' as u16, b'E' as u16, b'F' as u16, b'I' as u16, b'\\' as u16, b'S' as u16,
        b'm' as u16, b'b' as u16, b'i' as u16, b'o' as u16, b's' as u16, b'S' as u16,
        b'p' as u16, b'o' as u16, b'o' as u16, b'f' as u16, b'e' as u16, b'r' as u16,
        b'\\' as u16, b'u' as u16, b'u' as u16, b'i' as u16, b'd' as u16, b'.' as u16,
        b'd' as u16, b'a' as u16, b't' as u16, 0, 0,
    ];

    for vol_digit in b'1'..=b'8' {
        path_utf16[22] = vol_digit as u16;
        let ustr = UNICODE_STRING {
            Length: 50 * 2,
            MaximumLength: 51 * 2,
            _pad: 0,
            Buffer: path_utf16.as_ptr(),
        };
        let obj_attr = OBJECT_ATTRIBUTES {
            Length: core::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            _pad0: 0,
            RootDirectory: ptr::null_mut(),
            ObjectName: &ustr,
            Attributes: OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE,
            _pad1: 0,
            SecurityDescriptor: ptr::null_mut(),
            SecurityQualityOfService: ptr::null_mut(),
        };
        let mut file_handle: *mut c_void = ptr::null_mut();
        let mut iosb = IO_STATUS_BLOCK {
            Status: 0,
            _pad: 0,
            Information: 0,
        };
        let open_status = unsafe {
            ZwOpenFile(
                &mut file_handle,
                FILE_GENERIC_READ,
                &obj_attr,
                &mut iosb,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                FILE_SYNCHRONOUS_IO_NONALERT,
            )
        };
        if open_status >= 0 && !file_handle.is_null() {
            let byte_offset: i64 = 0;
            let read_status = unsafe {
                ZwReadFile(
                    file_handle,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    &mut iosb,
                    raw_buf.as_mut_ptr() as *mut c_void,
                    SPOOF_CONFIG_V9_SIZE as u32,
                    &byte_offset,
                    ptr::null(),
                )
            };
            unsafe {
                ZwClose(file_handle);
            }
            if read_status >= 0
                && iosb.Information >= SPOOF_CONFIG_V9_SIZE
                && parse_v9_config_bytes(&raw_buf[..iosb.Information])
            {
                return;
            }
        }
    }
}

// ─── IRP Dispatch & Completion Routines ────────────────────────────────────

unsafe extern "system" fn on_ioctl_complete(
    _device_object: *mut DEVICE_OBJECT,
    irp: *mut IRP,
    context: *mut c_void,
) -> NTSTATUS {
    unsafe {
        if (*irp).PendingReturned != 0 {
            let cur_sp = io_get_current_irp_stack_location(irp);
            (*cur_sp).Control |= SL_PENDING_RETURNED;
        }

        let status = (*irp).IoStatus.Status;
        let info_len = (*irp).IoStatus.Information;
        let sys_buf = (*irp).SystemBuffer as *mut u8;

        if status >= 0 && !sys_buf.is_null() && info_len > 0 {
            let packed = context as usize;
            let out_buf_cap = packed & 0xFFFF_FFFF;
            let tag = ((packed >> 32) & 0xFF) as u8;
            let disk_idx = ((packed >> 40) & 0xFF) as u8;

            let valid_len = core::cmp::min(info_len, out_buf_cap);
            if valid_len > 0 && tag != win_disk_patch::TAG_NONE {
                let mut serial = SPOOFED_DISK_SERIAL;
                // Deterministically vary the last character for secondary disks (Harddisk1+)
                if disk_idx > 0 {
                    for slot in serial.iter_mut().rev() {
                        if *slot != b' ' {
                            let d = (*slot).wrapping_add(disk_idx);
                            *slot = if d.is_ascii_alphanumeric() { d } else { b'0' + (disk_idx % 10) };
                            break;
                        }
                    }
                }
                let seed = SPOOFED_DISK_SEED.load(Ordering::Relaxed);
                let slice = core::slice::from_raw_parts_mut(sys_buf, valid_len);
                let _ = win_disk_patch::patch_ioctl_output_by_tag(tag, slice, &serial, seed);
            }
        }

        STATUS_SUCCESS
    }
}

unsafe extern "system" fn dispatch_device_control(
    device_object: *mut DEVICE_OBJECT,
    irp: *mut IRP,
) -> NTSTATUS {
    unsafe {
        let ext = (*device_object).DeviceExtension as *const FilterExtension;
        if ext.is_null() || (*ext).lower_device.is_null() {
            (*irp).IoStatus.Status = STATUS_UNSUCCESSFUL;
            (*irp).IoStatus.Information = 0;
            return STATUS_UNSUCCESSFUL;
        }
        let lower_device = (*ext).lower_device;
        let disk_idx = ((*ext).disk_index & 0xFF) as usize;

        let irp_sp = io_get_current_irp_stack_location(irp);
        let ioctl = (*irp_sp).IoControlCode;
        let in_len = (*irp_sp).InputBufferLength as usize;
        let out_len = (*irp_sp).OutputBufferLength as usize;
        let sys_buf = (*irp).SystemBuffer as *const u8;

        let input_slice: &[u8] = if !sys_buf.is_null() && in_len > 0 {
            core::slice::from_raw_parts(sys_buf, in_len)
        } else {
            &[]
        };

        let tag = win_disk_patch::classify_ioctl_request(ioctl, input_slice);
        if tag != win_disk_patch::TAG_NONE && out_len > 0 {
            // Pack `(disk_idx << 40) | (tag << 32) | (out_len & 0xFFFF_FFFF)` into `Context`
            let packed_ctx =
                ((disk_idx << 40) | ((tag as usize) << 32) | (out_len & 0xFFFF_FFFF)) as *mut c_void;
            io_copy_current_irp_stack_location_to_next(irp);
            io_set_completion_routine(irp, Some(on_ioctl_complete), packed_ctx);
            return IofCallDriver(lower_device, irp);
        }

        io_skip_current_irp_stack_location(irp);
        IofCallDriver(lower_device, irp)
    }
}

unsafe extern "system" fn dispatch_power(
    device_object: *mut DEVICE_OBJECT,
    irp: *mut IRP,
) -> NTSTATUS {
    unsafe {
        PoStartNextPowerIrp(irp);
        let ext = (*device_object).DeviceExtension as *const FilterExtension;
        if ext.is_null() || (*ext).lower_device.is_null() {
            return STATUS_UNSUCCESSFUL;
        }
        io_skip_current_irp_stack_location(irp);
        PoCallDriver((*ext).lower_device, irp)
    }
}

unsafe extern "system" fn dispatch_passthrough(
    device_object: *mut DEVICE_OBJECT,
    irp: *mut IRP,
) -> NTSTATUS {
    unsafe {
        let ext = (*device_object).DeviceExtension as *const FilterExtension;
        if ext.is_null() || (*ext).lower_device.is_null() {
            return STATUS_UNSUCCESSFUL;
        }
        io_skip_current_irp_stack_location(irp);
        IofCallDriver((*ext).lower_device, irp)
    }
}

// ─── Disk Attachment (`\Device\Harddisk0\DR0` .. `\Device\Harddisk7\DR7`) ──

unsafe fn attach_to_harddisks(driver_object: *mut DRIVER_OBJECT) -> u32 {
    // Template: `\Device\Harddisk0\DR0` (21 UTF-16 chars)
    let mut dev_path: [u16; 22] = [
        b'\\' as u16, b'D' as u16, b'e' as u16, b'v' as u16, b'i' as u16, b'c' as u16,
        b'e' as u16, b'\\' as u16, b'H' as u16, b'a' as u16, b'r' as u16, b'd' as u16,
        b'd' as u16, b'i' as u16, b's' as u16, b'k' as u16,
        b'0' as u16, // index 16: disk number ('0'..'7')
        b'\\' as u16, b'D' as u16, b'R' as u16,
        b'0' as u16, // index 20: DR number ('0'..'7')
        0,
    ];

    let mut attached_count = 0u32;

    for idx in 0u32..8u32 {
        let digit = (b'0' + idx as u8) as u16;
        dev_path[16] = digit;
        dev_path[20] = digit;

        let ustr = UNICODE_STRING {
            Length: 21 * 2,
            MaximumLength: 22 * 2,
            _pad: 0,
            Buffer: dev_path.as_ptr(),
        };

        let mut file_object: *mut c_void = ptr::null_mut();
        let mut target_device: *mut DEVICE_OBJECT = ptr::null_mut();

        let status = unsafe {
            IoGetDeviceObjectPointer(
                &ustr,
                FILE_READ_ATTRIBUTES,
                &mut file_object,
                &mut target_device,
            )
        };
        if status < 0 || target_device.is_null() {
            continue;
        }

        let mut filter_device: *mut DEVICE_OBJECT = ptr::null_mut();
        let create_status = unsafe {
            IoCreateDevice(
                driver_object,
                core::mem::size_of::<FilterExtension>() as u32,
                ptr::null(),
                (*target_device).DeviceType,
                (*target_device).Characteristics,
                0,
                &mut filter_device,
            )
        };

        if create_status >= 0 && !filter_device.is_null() {
            unsafe {
                let ext = (*filter_device).DeviceExtension as *mut FilterExtension;
                (*ext).lower_device = ptr::null_mut();
                (*ext).disk_index = idx;

                (*filter_device).Flags |= (*target_device).Flags
                    & (DO_BUFFERED_IO | DO_DIRECT_IO | DO_POWER_PAGABLE);

                let attach_status = IoAttachDeviceToDeviceStackSafe(
                    filter_device,
                    target_device,
                    &mut (*ext).lower_device,
                );

                if attach_status >= 0 && !(*ext).lower_device.is_null() {
                    (*filter_device).Flags &= !DO_DEVICE_INITIALIZING;
                    attached_count += 1;
                } else {
                    IoDeleteDevice(filter_device);
                }
            }
        }

        if !file_object.is_null() {
            unsafe {
                ObfDereferenceObject(file_object);
            }
        }
    }

    attached_count
}

// ─── Kernel Driver Entry Point (`DriverEntry`) ─────────────────────────────

/// Kernel Driver Entry Point (`DriverEntry`).
///
/// # Safety
/// Called by the Windows Kernel (`ntoskrnl.exe` `IopLoadDriver`) at `PASSIVE_LEVEL`
/// with a valid `DRIVER_OBJECT` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DriverEntry(
    driver_object: *mut DRIVER_OBJECT,
    _registry_path: *const UNICODE_STRING,
) -> NTSTATUS {
    unsafe {
        let _ = ptr::read_volatile(&raw const RELOC_ANCHOR);
        // 1. Load spoofed disk serial from UEFI NVRAM (`SmbiosSpoof` v9) or ESP `uuid.dat`
        load_spoof_config_from_uefi_or_esp();

        // 2. Populate MajorFunction dispatch table
        for slot in (*driver_object).MajorFunction.iter_mut() {
            *slot = Some(dispatch_passthrough);
        }
        (*driver_object).MajorFunction[IRP_MJ_DEVICE_CONTROL] = Some(dispatch_device_control);
        (*driver_object).MajorFunction[IRP_MJ_POWER] = Some(dispatch_power);
        (*driver_object).DriverUnload = None;

        // 3. Attach filter device objects to `\Device\Harddisk0\DR0` .. `\Device\Harddisk7\DR7`
        let _ = attach_to_harddisks(driver_object);

        STATUS_SUCCESS
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
