//! `smbios-disk-filter.sys` — Windows Kernel WDM Storage Filter Driver (`Ring 0`, `#![no_std]`).
//!
//! Synchronizes the NVMe/SATA/SCSI disk serial number generated in UEFI (`SmbiosSpoof` v9)
//! across the `ExitBootServices()` boundary into Windows (`stornvme.sys` / `disk.sys` / `spaceport.sys` / WMI).
//!
//! ### Architecture
//! 1. **Zero-WDK Compilation**: Uses MSVC `lib.exe` (`ntoskrnl.def`) to link against `ntoskrnl.exe`
//!    without requiring external C/C++ WDK libraries.
//! 2. **Multi-Source Profile Bridge (`v9`)**:
//!    - Source A: UEFI Runtime Variable `SmbiosSpoof` (`8BE4DF61-93CA-11D2-AA0D-00E098032B8C`)
//!      via `ExGetFirmwareEnvironmentVariable`.
//!    - Source B: Direct kernel read of the EFI System Partition file
//!      `\Device\HarddiskVolume1..8\EFI\SmbiosSpoofer\uuid.dat` via `ZwOpenFile` + `ZwReadFile`
//!      (works even when the ESP has no drive letter mounted in Windows!).
//! 3. **Automatic `DEVICEMAP\Scsi` Discovery & Registry Patch**:
//!    - Scans `\Registry\Machine\HARDWARE\DEVICEMAP\Scsi\Scsi Port 0..3\Scsi Bus 0..5\Target Id 0..1\Logical Unit Id 0`
//!      at `DriverEntry` (`PASSIVE_LEVEL`), captures the true factory disk serial into `ORIG_DISK_SERIAL`,
//!      and overwrites `SerialNumber` in `DEVICEMAP\Scsi` via `ZwSetValueKey`.
//! 4. **Multi-Stack Filter Attachment (`Harddisk0..7`, `RaidPort0..3`, `Spaceport`)**:
//!    - Attaches Filter Device Objects (`FiDO`) to `\Device\Harddisk0\DR0`..`DR7`, `\Device\RaidPort0..3`,
//!      and `\Device\Spaceport` (`Get-PhysicalDisk` / `MSFT_PhysicalDisk`).
//!    - Intercepts `IRP_MJ_DEVICE_CONTROL` completion for `IOCTL_STORAGE_QUERY_PROPERTY` (including
//!      `StorageDeviceProperty`, `StorageDeviceIdProperty`, `StorageDeviceUniqueIdProperty`),
//!      `IOCTL_STORAGE_PROTOCOL_COMMAND`, `SMART_RCV_DRIVE_DATA`, `IOCTL_ATA_PASS_THROUGH`,
//!      `IOCTL_SCSI_PASS_THROUGH`, and `\Device\Spaceport` enumeration buffers.

#![no_std]
#![no_main]
#![allow(non_snake_case, non_camel_case_types, dead_code)]

use core::ffi::c_void;
use core::panic::PanicInfo;
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

#[path = "../../src/win_disk_patch.rs"]
pub mod win_disk_patch;

// ─── Global Spoofed & Original Disk Serial State ───────────────────────────

static mut SPOOFED_DISK_SERIAL: [u8; 20] = *b"CA54N299714909Q56   ";
static mut SPOOFED_UUID_BYTES: [u8; 16] = [0u8; 16];
static HAS_SPOOFED_UUID: AtomicBool = AtomicBool::new(false);
static mut ORIG_DISK_SERIAL: [u8; 20] = [b' '; 20];
static HAS_ORIG_SERIAL: AtomicBool = AtomicBool::new(false);
static SPOOFED_DISK_SEED: AtomicU64 = AtomicU64::new(0x534D_4249_2026_0009);

const MAX_ATTACHED_FIDOS: usize = 16;
static mut ATTACHED_FIDOS: [*mut DEVICE_OBJECT; MAX_ATTACHED_FIDOS] =
    [ptr::null_mut(); MAX_ATTACHED_FIDOS];
static ATTACHED_COUNT: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_IRPS: AtomicUsize = AtomicUsize::new(0);
static UNLOADING: AtomicBool = AtomicBool::new(false);

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
pub const IRP_MJ_INTERNAL_DEVICE_CONTROL: usize = 0x0F;
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

pub const KEY_QUERY_VALUE: u32 = 0x0001;
pub const KEY_SET_VALUE: u32 = 0x0002;
pub const REG_SZ: u32 = 1;
pub const KEY_VALUE_PARTIAL_INFORMATION_TYPE: u32 = 2;

pub const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;
pub const OBJ_KERNEL_HANDLE: u32 = 0x0000_0200;

pub const SL_PENDING_RETURNED: u8 = 0x01;
pub const SL_INVOKE_ON_CANCEL: u8 = 0x20;
pub const SL_INVOKE_ON_SUCCESS: u8 = 0x40;
pub const SL_INVOKE_ON_ERROR: u8 = 0x80;

pub const MDL_MAPPED_TO_SYSTEM_VA: i16 = 0x0001;
pub const MDL_SOURCE_IS_NONPAGED_POOL: i16 = 0x0004;

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

/// `_MDL` on 64-bit Windows (`size = 0x30 = 48` bytes).
#[repr(C)]
pub struct MDL {
    pub Next: *mut MDL,
    pub Size: i16,
    pub MdlFlags: i16,
    pub Process: *mut c_void,
    pub MappedSystemVa: *mut c_void, // +0x18
    pub StartVa: *mut c_void,
    pub ByteCount: u32, // +0x28
    pub ByteOffset: u32,
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
#[repr(C)]
pub struct IO_STACK_LOCATION {
    pub MajorFunction: u8,
    pub MinorFunction: u8,
    pub Flags: u8,
    pub Control: u8,
    pub _pad0: u32,
    pub OutputBufferLength: u32,
    pub _pad1: u32,
    pub InputBufferLength: u32,
    pub _pad2: u32,
    pub IoControlCode: u32,
    pub _pad3: u32,
    pub Type3InputBuffer: *mut c_void,
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
    pub MdlAddress: *mut MDL,
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
    pub DriverContext: [*mut c_void; 4],
    pub Thread: *mut c_void,
    pub AuxiliaryBuffer: *mut u8,
    pub ListEntry: LIST_ENTRY,
    pub CurrentStackLocation: *mut IO_STACK_LOCATION, // +0xB8
    pub OriginalFileObject: *mut c_void,
    pub IrpExtension: *mut c_void,
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
    assert!(core::mem::offset_of!(MDL, MappedSystemVa) == 0x18);
    assert!(core::mem::offset_of!(MDL, ByteCount) == 0x28);
};

#[repr(C)]
struct FilterExtension {
    lower_device: *mut DEVICE_OBJECT,
    disk_index: u32,
}

// ─── `ntoskrnl.exe` Imports ────────────────────────────────────────────────

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
    fn IoDetachDevice(TargetDevice: *mut DEVICE_OBJECT);

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

    fn ZwOpenKey(
        KeyHandle: *mut *mut c_void,
        DesiredAccess: u32,
        ObjectAttributes: *const OBJECT_ATTRIBUTES,
    ) -> NTSTATUS;

    fn ZwQueryValueKey(
        KeyHandle: *mut c_void,
        ValueName: *const UNICODE_STRING,
        KeyValueInformationClass: u32,
        KeyValueInformation: *mut c_void,
        Length: u32,
        ResultLength: *mut u32,
    ) -> NTSTATUS;

    fn ZwSetValueKey(
        KeyHandle: *mut c_void,
        ValueName: *const UNICODE_STRING,
        TitleIndex: u32,
        Type: u32,
        Data: *const c_void,
        DataSize: u32,
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
        let mut uuid_16 = [0u8; 16];
        uuid_16.copy_from_slice(&raw[8..24]);
        if uuid_16 != [0u8; 16] {
            SPOOFED_UUID_BYTES = uuid_16;
            HAS_SPOOFED_UUID.store(true, Ordering::Release);
        }
    }
    SPOOFED_DISK_SEED.store(
        if seed != 0 { seed } else { 0x534D_4249 },
        Ordering::Relaxed,
    );
    true
}

unsafe fn load_spoof_config_from_uefi_or_esp() {
    let mut raw_buf = [0u8; 1728];

    // 1. Try UEFI Runtime Variable `SmbiosSpoof`
    const VAR_NAME_UTF16: [u16; 12] = [
        b'S' as u16, b'm' as u16, b'b' as u16, b'i' as u16, b'o' as u16, b's' as u16,
        b'S' as u16, b'p' as u16, b'o' as u16, b'o' as u16, b'f' as u16, 0,
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

/// Scans `\Registry\Machine\HARDWARE\DEVICEMAP\Scsi\Scsi Port P\Scsi Bus B\Target Id T\Logical Unit Id 0`
/// at `DriverEntry` (`PASSIVE_LEVEL`):
/// 1. Reads the existing `SerialNumber` value (`REG_SZ`) to capture `ORIG_DISK_SERIAL` (so substring
///    patching works on any NVMe/SATA SSD model automatically).
/// 2. Overwrites `SerialNumber` in `DEVICEMAP\Scsi` with `SPOOFED_DISK_SERIAL` (preserving `_00000001.` if present).
unsafe fn sync_devicemap_scsi_registry() {
    // Template (92 chars):
    // `\Registry\Machine\HARDWARE\DEVICEMAP\Scsi\Scsi Port 0\Scsi Bus 0\Target Id 0\Logical Unit Id 0`
    //  0123456789012345678901234567890123456789012345678901234567890123456789012345678901234567890123
    //                                                      ^52        ^63         ^75
    const TEMPLATE: &[u8; 94] =
        b"\\Registry\\Machine\\HARDWARE\\DEVICEMAP\\Scsi\\Scsi Port 0\\Scsi Bus 0\\Target Id 0\\Logical Unit Id 0";
    let mut key_utf16 = [0u16; 96];
    for (i, &b) in TEMPLATE.iter().enumerate() {
        key_utf16[i] = b as u16;
    }

    const VAL_SERIAL_UTF16: [u16; 13] = [
        b'S' as u16, b'e' as u16, b'r' as u16, b'i' as u16, b'a' as u16, b'l' as u16,
        b'N' as u16, b'u' as u16, b'm' as u16, b'b' as u16, b'e' as u16, b'r' as u16, 0,
    ];
    let val_ustr = UNICODE_STRING {
        Length: 12 * 2,
        MaximumLength: 13 * 2,
        _pad: 0,
        Buffer: VAL_SERIAL_UTF16.as_ptr(),
    };

    // Buffer for `KEY_VALUE_PARTIAL_INFORMATION` (12-byte header: TitleIndex u32, Type u32, DataLength u32, Data[...])
    let mut info_buf = [0u8; 256];

    for port in b'0'..=b'3' {
        key_utf16[52] = port as u16;
        for bus in b'0'..=b'5' {
            key_utf16[63] = bus as u16;
            for target in b'0'..=b'1' {
                key_utf16[75] = target as u16;

                let key_ustr = UNICODE_STRING {
                    Length: 94 * 2,
                    MaximumLength: 95 * 2,
                    _pad: 0,
                    Buffer: key_utf16.as_ptr(),
                };
                let obj_attr = OBJECT_ATTRIBUTES {
                    Length: core::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
                    _pad0: 0,
                    RootDirectory: ptr::null_mut(),
                    ObjectName: &key_ustr,
                    Attributes: OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE,
                    _pad1: 0,
                    SecurityDescriptor: ptr::null_mut(),
                    SecurityQualityOfService: ptr::null_mut(),
                };

                let mut key_handle: *mut c_void = ptr::null_mut();
                let status = unsafe {
                    ZwOpenKey(&mut key_handle, KEY_QUERY_VALUE | KEY_SET_VALUE, &obj_attr)
                };
                if status < 0 || key_handle.is_null() {
                    continue;
                }

                let mut res_len: u32 = 0;
                let q_status = unsafe {
                    ZwQueryValueKey(
                        key_handle,
                        &val_ustr,
                        KEY_VALUE_PARTIAL_INFORMATION_TYPE,
                        info_buf.as_mut_ptr() as *mut c_void,
                        info_buf.len() as u32,
                        &mut res_len,
                    )
                };

                if q_status >= 0 && res_len >= 16 {
                    let data_len =
                        u32::from_le_bytes([info_buf[8], info_buf[9], info_buf[10], info_buf[11]])
                            as usize;
                    if data_len >= 8 && 12 + data_len <= info_buf.len() {
                        // Extract ASCII characters from UTF-16LE `Data`
                        let char_count = core::cmp::min(data_len / 2, 60);
                        let mut ascii_tmp = [0u8; 64];
                        let mut actual_chars = 0usize;
                        for k in 0..char_count {
                            let ch = u16::from_le_bytes([
                                info_buf[12 + k * 2],
                                info_buf[12 + k * 2 + 1],
                            ]);
                            if ch == 0 {
                                break;
                            }
                            if (0x20..=0x7E).contains(&ch) {
                                ascii_tmp[actual_chars] = ch as u8;
                                actual_chars += 1;
                            }
                        }

                        if actual_chars >= 6 {
                            let spoof = unsafe { SPOOFED_DISK_SERIAL };
                            // Capture `ORIG_DISK_SERIAL` if it differs from `SPOOFED_DISK_SERIAL`
                            if !HAS_ORIG_SERIAL.load(Ordering::Relaxed) {
                                let mut orig_20 = [b' '; 20];
                                let copy_n = core::cmp::min(actual_chars, 20);
                                orig_20[..copy_n].copy_from_slice(&ascii_tmp[..copy_n]);
                                if orig_20 != spoof {
                                    unsafe {
                                        ORIG_DISK_SERIAL = orig_20;
                                    }
                                    HAS_ORIG_SERIAL.store(true, Ordering::Release);
                                }
                            }

                            // Patch `ascii_tmp[..actual_chars]` in-place (preserving `_00000001.` suffix)
                            win_disk_patch::patch_ascii_serial_slice(
                                &mut ascii_tmp[..actual_chars],
                                &spoof,
                            );

                            // Encode back to UTF-16LE and write via `ZwSetValueKey`
                            let mut new_utf16 = [0u16; 64];
                            for k in 0..actual_chars {
                                new_utf16[k] = ascii_tmp[k] as u16;
                            }
                            new_utf16[actual_chars] = 0;
                            let write_bytes = ((actual_chars + 1) * 2) as u32;
                            unsafe {
                                let _ = ZwSetValueKey(
                                    key_handle,
                                    &val_ustr,
                                    0,
                                    REG_SZ,
                                    new_utf16.as_ptr() as *const c_void,
                                    write_bytes,
                                );
                            }
                        }
                    }
                }

                unsafe {
                    ZwClose(key_handle);
                }
            }
        }
    }
}

/// Writes a UTF-16LE `REG_SZ` value to a kernel Registry path (`\Registry\Machine\...`).
unsafe fn write_registry_sz_ascii(key_ascii: &[u8], val_ascii: &[u8], data_ascii: &[u8]) {
    if key_ascii.len() > 96 || val_ascii.len() > 32 || data_ascii.len() > 60 {
        return;
    }
    let mut key_u16 = [0u16; 98];
    for (i, &b) in key_ascii.iter().enumerate() {
        key_u16[i] = b as u16;
    }
    let mut val_u16 = [0u16; 34];
    for (i, &b) in val_ascii.iter().enumerate() {
        val_u16[i] = b as u16;
    }
    let mut data_u16 = [0u16; 64];
    for (i, &b) in data_ascii.iter().enumerate() {
        data_u16[i] = b as u16;
    }
    data_u16[data_ascii.len()] = 0;

    let key_ustr = UNICODE_STRING {
        Length: (key_ascii.len() * 2) as u16,
        MaximumLength: ((key_ascii.len() + 1) * 2) as u16,
        _pad: 0,
        Buffer: key_u16.as_ptr(),
    };
    let val_ustr = UNICODE_STRING {
        Length: (val_ascii.len() * 2) as u16,
        MaximumLength: ((val_ascii.len() + 1) * 2) as u16,
        _pad: 0,
        Buffer: val_u16.as_ptr(),
    };
    let obj_attr = OBJECT_ATTRIBUTES {
        Length: core::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        _pad0: 0,
        RootDirectory: ptr::null_mut(),
        ObjectName: &key_ustr,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_KERNEL_HANDLE,
        _pad1: 0,
        SecurityDescriptor: ptr::null_mut(),
        SecurityQualityOfService: ptr::null_mut(),
    };

    let mut key_handle: *mut c_void = ptr::null_mut();
    let status = unsafe { ZwOpenKey(&mut key_handle, KEY_SET_VALUE, &obj_attr) };
    if status >= 0 && !key_handle.is_null() {
        let data_bytes = ((data_ascii.len() + 1) * 2) as u32;
        unsafe {
            let _ = ZwSetValueKey(
                key_handle,
                &val_ustr,
                0,
                REG_SZ,
                data_u16.as_ptr() as *const c_void,
                data_bytes,
            );
            ZwClose(key_handle);
        }
    }
}

/// Synchronizes `MachineGuid`, `ComputerHardwareId`, and `MachineId` in the Windows Registry
/// at every boot from the spoofed SMBIOS UUID (`SmbiosSpoof` v9).
unsafe fn sync_os_telemetry_guids_from_uuid() {
    if !HAS_SPOOFED_UUID.load(Ordering::Acquire) {
        return;
    }
    let u = unsafe { SPOOFED_UUID_BYTES };
    const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";
    const HEX_UPPER: &[u8; 16] = b"0123456789ABCDEF";

    // Format `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` (36 bytes) and `{...}` (38 bytes)
    let mut lower_36 = [0u8; 36];
    let mut braced_lower_38 = [0u8; 38];
    let mut braced_upper_38 = [0u8; 38];
    braced_lower_38[0] = b'{';
    braced_upper_38[0] = b'{';

    let mut out = 0usize;
    for (idx, &byte) in u.iter().enumerate() {
        if idx == 4 || idx == 6 || idx == 8 || idx == 10 {
            lower_36[out] = b'-';
            braced_lower_38[out + 1] = b'-';
            braced_upper_38[out + 1] = b'-';
            out += 1;
        }
        let hi = (byte >> 4) as usize;
        let lo = (byte & 0x0F) as usize;
        lower_36[out] = HEX_LOWER[hi];
        lower_36[out + 1] = HEX_LOWER[lo];
        braced_lower_38[out + 1] = HEX_LOWER[hi];
        braced_lower_38[out + 2] = HEX_LOWER[lo];
        braced_upper_38[out + 1] = HEX_UPPER[hi];
        braced_upper_38[out + 2] = HEX_UPPER[lo];
        out += 2;
    }
    braced_lower_38[37] = b'}';
    braced_upper_38[37] = b'}';

    unsafe {
        write_registry_sz_ascii(
            b"\\Registry\\Machine\\SOFTWARE\\Microsoft\\Cryptography",
            b"MachineGuid",
            &lower_36,
        );
        write_registry_sz_ascii(
            b"\\Registry\\Machine\\SYSTEM\\CurrentControlSet\\Control\\SystemInformation",
            b"ComputerHardwareId",
            &braced_lower_38,
        );
        write_registry_sz_ascii(
            b"\\Registry\\Machine\\SOFTWARE\\Microsoft\\SQMClient",
            b"MachineId",
            &braced_upper_38,
        );
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

        if status >= 0 && info_len > 0 {
            let packed = context as usize;
            let out_buf_cap = packed & 0xFFFF_FFFF;
            let tag = ((packed >> 32) & 0xFF) as u8;
            let disk_idx = ((packed >> 40) & 0xFF) as u8;

            let valid_len = core::cmp::min(info_len, out_buf_cap);
            if valid_len > 0 && tag != win_disk_patch::TAG_NONE {
                let mut serial = SPOOFED_DISK_SERIAL;
                if disk_idx > 0 && disk_idx < 8 {
                    for slot in serial.iter_mut().rev() {
                        if *slot != b' ' {
                            let d = (*slot).wrapping_add(disk_idx);
                            *slot = if d.is_ascii_alphanumeric() {
                                d
                            } else {
                                b'0' + (disk_idx % 10)
                            };
                            break;
                        }
                    }
                }
                let seed = SPOOFED_DISK_SEED.load(Ordering::Relaxed);
                let has_orig = HAS_ORIG_SERIAL.load(Ordering::Acquire);
                let orig = ORIG_DISK_SERIAL;

                // 1. Patch `SystemBuffer` (`METHOD_BUFFERED`)
                let sys_buf = (*irp).SystemBuffer as *mut u8;
                if !sys_buf.is_null() {
                    let slice = core::slice::from_raw_parts_mut(sys_buf, valid_len);
                    let _ = win_disk_patch::patch_ioctl_output_by_tag(tag, slice, &serial, seed);
                    if has_orig {
                        let _ = win_disk_patch::patch_buffer_occurrences_ascii_and_utf16(
                            slice, &orig, &serial,
                        );
                    }
                }

                // 2. Patch `MdlAddress->MappedSystemVa` if mapped in NonPaged system space (`METHOD_OUT_DIRECT`)
                let mdl = (*irp).MdlAddress;
                if !mdl.is_null()
                    && ((*mdl).MdlFlags & (MDL_MAPPED_TO_SYSTEM_VA | MDL_SOURCE_IS_NONPAGED_POOL))
                        != 0
                    && !(*mdl).MappedSystemVa.is_null()
                    && (*mdl).ByteCount > 0
                {
                    let mdl_len = core::cmp::min(valid_len, (*mdl).ByteCount as usize);
                    let mdl_slice =
                        core::slice::from_raw_parts_mut((*mdl).MappedSystemVa as *mut u8, mdl_len);
                    let _ =
                        win_disk_patch::patch_ioctl_output_by_tag(tag, mdl_slice, &serial, seed);
                    if has_orig {
                        let _ = win_disk_patch::patch_buffer_occurrences_ascii_and_utf16(
                            mdl_slice, &orig, &serial,
                        );
                    }
                }
            }
        }

        ACTIVE_IRPS.fetch_sub(1, Ordering::AcqRel);
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

        if !UNLOADING.load(Ordering::Acquire) {
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
                ACTIVE_IRPS.fetch_add(1, Ordering::AcqRel);
                let packed_ctx = ((disk_idx << 40)
                    | ((tag as usize) << 32)
                    | (out_len & 0xFFFF_FFFF)) as *mut c_void;
                io_copy_current_irp_stack_location_to_next(irp);
                io_set_completion_routine(irp, Some(on_ioctl_complete), packed_ctx);
                return IofCallDriver(lower_device, irp);
            }
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

#[allow(clippy::needless_range_loop)]
unsafe extern "system" fn driver_unload(_driver_object: *mut DRIVER_OBJECT) {
    UNLOADING.store(true, Ordering::Release);
    let count = ATTACHED_COUNT.load(Ordering::Acquire);

    unsafe {
        for i in 0..count {
            let fido = ATTACHED_FIDOS[i];
            if !fido.is_null() {
                let ext = (*fido).DeviceExtension as *const FilterExtension;
                if !ext.is_null() && !(*ext).lower_device.is_null() {
                    IoDetachDevice((*ext).lower_device);
                }
            }
        }

        // Wait for any in-flight completion routines to drain
        let mut spins = 0usize;
        while ACTIVE_IRPS.load(Ordering::Acquire) > 0 && spins < 50_000_000 {
            core::hint::spin_loop();
            spins += 1;
        }

        for i in 0..count {
            let fido = ATTACHED_FIDOS[i];
            if !fido.is_null() {
                IoDeleteDevice(fido);
                ATTACHED_FIDOS[i] = ptr::null_mut();
            }
        }
    }
}

// ─── Device Attachment Helper ──────────────────────────────────────────────

unsafe fn attach_single_device_path(
    driver_object: *mut DRIVER_OBJECT,
    ustr: &UNICODE_STRING,
    disk_index: u32,
) -> bool {
    let slot = ATTACHED_COUNT.load(Ordering::Relaxed);
    if slot >= MAX_ATTACHED_FIDOS {
        return false;
    }

    let mut file_object: *mut c_void = ptr::null_mut();
    let mut target_device: *mut DEVICE_OBJECT = ptr::null_mut();

    let status = unsafe {
        IoGetDeviceObjectPointer(
            ustr,
            FILE_READ_ATTRIBUTES,
            &mut file_object,
            &mut target_device,
        )
    };
    if status < 0 || target_device.is_null() {
        return false;
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

    let mut attached = false;
    if create_status >= 0 && !filter_device.is_null() {
        unsafe {
            let ext = (*filter_device).DeviceExtension as *mut FilterExtension;
            (*ext).lower_device = ptr::null_mut();
            (*ext).disk_index = disk_index;

            (*filter_device).Flags |=
                (*target_device).Flags & (DO_BUFFERED_IO | DO_DIRECT_IO | DO_POWER_PAGABLE);

            let attach_status = IoAttachDeviceToDeviceStackSafe(
                filter_device,
                target_device,
                &mut (*ext).lower_device,
            );

            if attach_status >= 0 && !(*ext).lower_device.is_null() {
                (*filter_device).Flags &= !DO_DEVICE_INITIALIZING;
                ATTACHED_FIDOS[slot] = filter_device;
                ATTACHED_COUNT.store(slot + 1, Ordering::Release);
                attached = true;
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
    attached
}

unsafe fn attach_all_storage_targets(driver_object: *mut DRIVER_OBJECT) {
    // 1. `\Device\Harddisk0\DR0` .. `\Device\Harddisk7\DR7` (PhysicalDrive0..7)
    let mut hd_path: [u16; 22] = [
        b'\\' as u16, b'D' as u16, b'e' as u16, b'v' as u16, b'i' as u16, b'c' as u16,
        b'e' as u16, b'\\' as u16, b'H' as u16, b'a' as u16, b'r' as u16, b'd' as u16,
        b'd' as u16, b'i' as u16, b's' as u16, b'k' as u16,
        b'0' as u16, // index 16
        b'\\' as u16, b'D' as u16, b'R' as u16,
        b'0' as u16, // index 20
        0,
    ];
    for idx in 0u32..8u32 {
        let d = (b'0' + idx as u8) as u16;
        hd_path[16] = d;
        hd_path[20] = d;
        let ustr = UNICODE_STRING {
            Length: 21 * 2,
            MaximumLength: 22 * 2,
            _pad: 0,
            Buffer: hd_path.as_ptr(),
        };
        unsafe {
            let _ = attach_single_device_path(driver_object, &ustr, idx);
        }
    }

    // 2. `\Device\RaidPort0` .. `\Device\RaidPort3` (Scsi0:..Scsi3:)
    let mut rp_path: [u16; 18] = [
        b'\\' as u16, b'D' as u16, b'e' as u16, b'v' as u16, b'i' as u16, b'c' as u16,
        b'e' as u16, b'\\' as u16, b'R' as u16, b'a' as u16, b'i' as u16, b'd' as u16,
        b'P' as u16, b'o' as u16, b'r' as u16, b't' as u16,
        b'0' as u16, // index 16
        0,
    ];
    for idx in 0u32..4u32 {
        rp_path[16] = (b'0' + idx as u8) as u16;
        let ustr = UNICODE_STRING {
            Length: 17 * 2,
            MaximumLength: 18 * 2,
            _pad: 0,
            Buffer: rp_path.as_ptr(),
        };
        unsafe {
            let _ = attach_single_device_path(driver_object, &ustr, 0);
        }
    }

    // 3. `\Device\Spaceport` (Storage Spaces control device backing `Get-PhysicalDisk`)
    const SPACEPORT_PATH: [u16; 18] = [
        b'\\' as u16, b'D' as u16, b'e' as u16, b'v' as u16, b'i' as u16, b'c' as u16,
        b'e' as u16, b'\\' as u16, b'S' as u16, b'p' as u16, b'a' as u16, b'c' as u16,
        b'e' as u16, b'p' as u16, b'o' as u16, b'r' as u16, b't' as u16, 0,
    ];
    let sp_ustr = UNICODE_STRING {
        Length: 17 * 2,
        MaximumLength: 18 * 2,
        _pad: 0,
        Buffer: SPACEPORT_PATH.as_ptr(),
    };
    unsafe {
        let _ = attach_single_device_path(driver_object, &sp_ustr, 0);
    }
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

        // 2. Capture factory serial from `HKLM\HARDWARE\DEVICEMAP\Scsi` and overwrite it in Registry
        sync_devicemap_scsi_registry();
        sync_os_telemetry_guids_from_uuid();

        // 3. Populate MajorFunction dispatch table
        for slot in (*driver_object).MajorFunction.iter_mut() {
            *slot = Some(dispatch_passthrough);
        }
        (*driver_object).MajorFunction[IRP_MJ_DEVICE_CONTROL] = Some(dispatch_device_control);
        (*driver_object).MajorFunction[IRP_MJ_POWER] = Some(dispatch_power);
        (*driver_object).DriverUnload = Some(driver_unload);

        // 4. Attach FiDOs to Harddisk0..7, RaidPort0..3, and Spaceport
        attach_all_storage_targets(driver_object);

        STATUS_SUCCESS
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
