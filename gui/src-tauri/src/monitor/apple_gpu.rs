#![allow(non_upper_case_globals)]

use std::{
    collections::HashMap,
    marker::{PhantomData, PhantomPinned},
    mem::{size_of, MaybeUninit},
    os::raw::c_void,
    ptr::null,
};

use core_foundation::{
    array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
    base::{CFAllocatorRef, CFRange, CFRelease, CFTypeRef, kCFAllocatorDefault, kCFAllocatorNull},
    data::{CFDataGetBytes, CFDataGetLength, CFDataRef},
    dictionary::{
        kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryCreate,
        CFDictionaryCreateMutableCopy, CFDictionaryGetCount, CFDictionaryGetValue,
        CFDictionaryRef, CFMutableDictionaryRef,
    },
    number::{kCFNumberSInt32Type, CFNumberCreate, CFNumberRef},
    string::{
        kCFStringEncodingUTF8, CFStringCreateWithBytesNoCopy, CFStringGetCString, CFStringRef,
    },
};

use super::types::GpuMetrics;

type CVoidRef = *const std::ffi::c_void;

// ─── CF Utilities ───────────────────────────────────────────────────────────

fn cfnum(val: i32) -> CFNumberRef {
    unsafe { CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &val as *const i32 as _) }
}

fn cfstr(val: &str) -> CFStringRef {
    unsafe {
        CFStringCreateWithBytesNoCopy(
            kCFAllocatorDefault,
            val.as_ptr(),
            val.len() as isize,
            kCFStringEncodingUTF8,
            0,
            kCFAllocatorNull,
        )
    }
}

fn from_cfstr(val: CFStringRef) -> String {
    unsafe {
        let mut buf = Vec::with_capacity(128);
        if CFStringGetCString(val, buf.as_mut_ptr(), 128, kCFStringEncodingUTF8) == 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .to_string()
    }
}

fn cfdict_get_val(dict: CFDictionaryRef, key: &str) -> Option<CFTypeRef> {
    unsafe {
        let key = cfstr(key);
        let val = CFDictionaryGetValue(dict, key as _);
        CFRelease(key as _);
        if val.is_null() {
            None
        } else {
            Some(val)
        }
    }
}

// ─── IOKit FFI ──────────────────────────────────────────────────────────────

#[link(name = "IOKit", kind = "framework")]
#[rustfmt::skip]
unsafe extern "C" {
    fn IOServiceMatching(name: *const i8) -> CFMutableDictionaryRef;
    fn IOServiceGetMatchingServices(mainPort: u32, matching: CFDictionaryRef, existing: *mut u32) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IORegistryEntryGetName(entry: u32, name: *mut i8) -> i32;
    fn IORegistryEntryCreateCFProperties(entry: u32, properties: *mut CFMutableDictionaryRef, allocator: CFAllocatorRef, options: u32) -> i32;
    fn IOObjectRelease(obj: u32) -> u32;
}

// ─── IOReport FFI ───────────────────────────────────────────────────────────

#[repr(C)]
struct IOReportSubscription {
    _data: [u8; 0],
    _phantom: PhantomData<(*mut u8, PhantomPinned)>,
}

type IOReportSubscriptionRef = *const IOReportSubscription;

#[link(name = "IOReport", kind = "dylib")]
#[rustfmt::skip]
unsafe extern "C" {
    fn IOReportCopyChannelsInGroup(a: CFStringRef, b: CFStringRef, c: u64, d: u64, e: u64) -> CFDictionaryRef;
    fn IOReportMergeChannels(a: CFDictionaryRef, b: CFDictionaryRef, nil: CFTypeRef);
    fn IOReportCreateSubscription(a: CVoidRef, b: CFMutableDictionaryRef, c: *mut CFMutableDictionaryRef, d: u64, e: CFTypeRef) -> IOReportSubscriptionRef;
    fn IOReportCreateSamples(a: IOReportSubscriptionRef, b: CFMutableDictionaryRef, c: CFTypeRef) -> CFDictionaryRef;
    fn IOReportCreateSamplesDelta(a: CFDictionaryRef, b: CFDictionaryRef, c: CFTypeRef) -> CFDictionaryRef;
    fn IOReportChannelGetGroup(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetSubGroup(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportChannelGetChannelName(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportSimpleGetIntegerValue(a: CFDictionaryRef, b: i32) -> i64;
    fn IOReportChannelGetUnitLabel(a: CFDictionaryRef) -> CFStringRef;
    fn IOReportStateGetCount(a: CFDictionaryRef) -> i32;
    fn IOReportStateGetNameForIndex(a: CFDictionaryRef, b: i32) -> CFStringRef;
    fn IOReportStateGetResidency(a: CFDictionaryRef, b: i32) -> i64;
}

// ─── IOReport Helpers ───────────────────────────────────────────────────────

fn cfio_get_group(item: CFDictionaryRef) -> String {
    match unsafe { IOReportChannelGetGroup(item) } {
        x if x.is_null() => String::new(),
        x => from_cfstr(x),
    }
}

fn cfio_get_subgroup(item: CFDictionaryRef) -> String {
    match unsafe { IOReportChannelGetSubGroup(item) } {
        x if x.is_null() => String::new(),
        x => from_cfstr(x),
    }
}

fn cfio_get_channel(item: CFDictionaryRef) -> String {
    match unsafe { IOReportChannelGetChannelName(item) } {
        x if x.is_null() => String::new(),
        x => from_cfstr(x),
    }
}

fn cfio_get_residencies(item: CFDictionaryRef) -> Vec<(String, i64)> {
    let count = unsafe { IOReportStateGetCount(item) };
    let mut res = Vec::new();
    for i in 0..count {
        let name = unsafe { IOReportStateGetNameForIndex(item, i) };
        let val = unsafe { IOReportStateGetResidency(item, i) };
        res.push((from_cfstr(name), val));
    }
    res
}

fn cfio_watts(item: CFDictionaryRef, unit: &str, duration_ms: u64) -> Option<f32> {
    let val = unsafe { IOReportSimpleGetIntegerValue(item, 0) } as f32;
    let val = val / (duration_ms as f32 / 1000.0);
    match unit {
        "mJ" => Some(val / 1e3),
        "uJ" => Some(val / 1e6),
        "nJ" => Some(val / 1e9),
        _ => None,
    }
}

// ─── IOReport Iterator ─────────────────────────────────────────────────────

struct IOReportIterator {
    sample: CFDictionaryRef,
    index: isize,
    items: CFArrayRef,
    items_size: isize,
}

impl IOReportIterator {
    fn new(data: CFDictionaryRef) -> Self {
        let items = cfdict_get_val(data, "IOReportChannels").unwrap() as CFArrayRef;
        let items_size = unsafe { CFArrayGetCount(items) } as isize;
        Self {
            sample: data,
            items,
            items_size,
            index: 0,
        }
    }
}

impl Drop for IOReportIterator {
    fn drop(&mut self) {
        unsafe { CFRelease(self.sample as _) };
    }
}

struct IOReportItem {
    group: String,
    subgroup: String,
    channel: String,
    unit: String,
    item: CFDictionaryRef,
}

impl Iterator for IOReportIterator {
    type Item = IOReportItem;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.items_size {
            return None;
        }
        let item =
            unsafe { CFArrayGetValueAtIndex(self.items, self.index) } as CFDictionaryRef;
        let group = cfio_get_group(item);
        let subgroup = cfio_get_subgroup(item);
        let channel = cfio_get_channel(item);
        let unit = from_cfstr(unsafe { IOReportChannelGetUnitLabel(item) })
            .trim()
            .to_string();
        self.index += 1;
        Some(IOReportItem {
            group,
            subgroup,
            channel,
            unit,
            item,
        })
    }
}

// ─── IOService Iterator ────────────────────────────────────────────────────

struct IOServiceIterator {
    existing: u32,
}

impl IOServiceIterator {
    fn new(service_name: &str) -> Option<Self> {
        let service_name = std::ffi::CString::new(service_name).ok()?;
        let existing = unsafe {
            let service = IOServiceMatching(service_name.as_ptr());
            let mut existing = 0u32;
            if IOServiceGetMatchingServices(0, service, &mut existing) != 0 {
                return None;
            }
            existing
        };
        Some(Self { existing })
    }
}

impl Drop for IOServiceIterator {
    fn drop(&mut self) {
        unsafe {
            IOObjectRelease(self.existing);
        }
    }
}

impl Iterator for IOServiceIterator {
    type Item = (u32, String);

    fn next(&mut self) -> Option<Self::Item> {
        let next = unsafe { IOIteratorNext(self.existing) };
        if next == 0 {
            return None;
        }
        let mut name = [0i8; 128];
        if unsafe { IORegistryEntryGetName(next, name.as_mut_ptr()) } != 0 {
            return None;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
        let name = name.to_string_lossy().to_string();
        Some((next, name))
    }
}

// ─── IOReport Subscription ─────────────────────────────────────────────────

struct IOReportSub {
    subs: IOReportSubscriptionRef,
    chan: CFMutableDictionaryRef,
}

impl IOReportSub {
    fn new(channels: &[(&str, Option<&str>)]) -> Option<Self> {
        let mut chan_dicts = Vec::new();
        for (group, subgroup) in channels {
            let gname = cfstr(group);
            let sname = subgroup.map_or(null(), cfstr);
            let chan = unsafe { IOReportCopyChannelsInGroup(gname, sname, 0, 0, 0) };
            if chan.is_null() {
                unsafe { CFRelease(gname as _) };
                if subgroup.is_some() {
                    unsafe { CFRelease(sname as _) };
                }
                continue;
            }
            chan_dicts.push(chan);
            unsafe { CFRelease(gname as _) };
            if subgroup.is_some() {
                unsafe { CFRelease(sname as _) };
            }
        }

        if chan_dicts.is_empty() {
            return None;
        }

        let first = chan_dicts[0];
        for i in 1..chan_dicts.len() {
            unsafe { IOReportMergeChannels(first, chan_dicts[i], null()) };
        }

        let size = unsafe { CFDictionaryGetCount(first) };
        let chan = unsafe { CFDictionaryCreateMutableCopy(kCFAllocatorDefault, size, first) };

        for d in &chan_dicts {
            unsafe { CFRelease(*d as _) };
        }

        if cfdict_get_val(chan, "IOReportChannels").is_none() {
            unsafe { CFRelease(chan as _) };
            return None;
        }

        let mut s: MaybeUninit<CFMutableDictionaryRef> = MaybeUninit::uninit();
        let subs =
            unsafe { IOReportCreateSubscription(null(), chan, s.as_mut_ptr(), 0, null()) };
        if subs.is_null() {
            unsafe { CFRelease(chan as _) };
            return None;
        }
        unsafe { s.assume_init() };

        Some(Self { subs, chan })
    }

    fn sample(&self) -> CFDictionaryRef {
        unsafe { IOReportCreateSamples(self.subs, self.chan, null()) }
    }
}

impl Drop for IOReportSub {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.chan as _);
            // subs is an opaque ref managed by IOReport; do not CFRelease
        }
    }
}

// ─── SMC FFI ────────────────────────────────────────────────────────────────

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn mach_task_self() -> u32;
    fn IOServiceOpen(device: u32, a: u32, b: u32, c: *mut u32) -> i32;
    fn IOServiceClose(conn: u32) -> i32;
    fn IOConnectCallStructMethod(
        conn: u32,
        selector: u32,
        ival: *const c_void,
        isize: usize,
        oval: *mut c_void,
        osize: *mut usize,
    ) -> i32;
}

#[repr(C)]
#[derive(Debug, Default)]
struct KeyDataVer {
    major: u8,
    minor: u8,
    build: u8,
    reserved: u8,
    release: u16,
}

#[repr(C)]
#[derive(Debug, Default)]
struct PLimitData {
    version: u16,
    length: u16,
    cpu_p_limit: u32,
    gpu_p_limit: u32,
    mem_p_limit: u32,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct KeyInfo {
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
}

#[repr(C)]
#[derive(Debug, Default)]
struct KeyData {
    key: u32,
    vers: KeyDataVer,
    p_limit_data: PLimitData,
    key_info: KeyInfo,
    result: u8,
    status: u8,
    data8: u8,
    data32: u32,
    bytes: [u8; 32],
}

struct SmcConnection {
    conn: u32,
    keys: HashMap<u32, KeyInfo>,
}

impl SmcConnection {
    fn new() -> Option<Self> {
        let mut conn = 0u32;
        for (device, name) in IOServiceIterator::new("AppleSMC")? {
            if name == "AppleSMCKeysEndpoint" {
                let rs = unsafe { IOServiceOpen(device, mach_task_self(), 0, &mut conn) };
                if rs != 0 {
                    return None;
                }
            }
        }
        if conn == 0 {
            return None;
        }
        Some(Self {
            conn,
            keys: HashMap::new(),
        })
    }

    fn read(&self, input: &KeyData) -> Option<KeyData> {
        let ival = input as *const _ as _;
        let ilen = size_of::<KeyData>();
        let mut oval = KeyData::default();
        let mut olen = size_of::<KeyData>();

        let rs = unsafe {
            IOConnectCallStructMethod(
                self.conn,
                2,
                ival,
                ilen,
                &mut oval as *mut _ as _,
                &mut olen,
            )
        };
        if rs != 0 || oval.result != 0 {
            return None;
        }
        Some(oval)
    }

    fn key_by_index(&self, index: u32) -> Option<String> {
        let ival = KeyData {
            data8: 8,
            data32: index,
            ..Default::default()
        };
        let oval = self.read(&ival)?;
        Some(
            std::str::from_utf8(&oval.key.to_be_bytes())
                .unwrap_or("")
                .to_string(),
        )
    }

    fn read_key_info(&mut self, key: &str) -> Option<KeyInfo> {
        if key.len() != 4 {
            return None;
        }
        let key_u32 = key.bytes().fold(0u32, |acc, x| (acc << 8) + x as u32);
        if let Some(ki) = self.keys.get(&key_u32) {
            return Some(*ki);
        }
        let ival = KeyData {
            data8: 9,
            key: key_u32,
            ..Default::default()
        };
        let oval = self.read(&ival)?;
        self.keys.insert(key_u32, oval.key_info);
        Some(oval.key_info)
    }

    fn read_val(&mut self, key: &str) -> Option<Vec<u8>> {
        let key_info = self.read_key_info(key)?;
        let key_u32 = key.bytes().fold(0u32, |acc, x| (acc << 8) + x as u32);
        let ival = KeyData {
            data8: 5,
            key: key_u32,
            key_info,
            ..Default::default()
        };
        let oval = self.read(&ival)?;
        Some(oval.bytes[0..key_info.data_size as usize].to_vec())
    }

    fn find_gpu_temp_keys(&mut self) -> Vec<String> {
        const FLOAT_TYPE: u32 = 1718383648; // FourCC: "flt "
        let mut gpu_keys = Vec::new();

        let key_count = match self.read_val("#KEY") {
            Some(data) if data.len() >= 4 => {
                u32::from_be_bytes(data[0..4].try_into().unwrap())
            }
            _ => return gpu_keys,
        };

        for i in 0..key_count {
            let key = match self.key_by_index(i) {
                Some(k) => k,
                None => continue,
            };
            if !key.starts_with("Tg") {
                continue;
            }
            let info = match self.read_key_info(&key) {
                Some(i) => i,
                None => continue,
            };
            if info.data_size == 4 && info.data_type == FLOAT_TYPE {
                if self.read_val(&key).is_some() {
                    gpu_keys.push(key);
                }
            }
        }
        gpu_keys
    }

    fn read_temp(&mut self, key: &str) -> Option<f32> {
        let data = self.read_val(key)?;
        if data.len() == 4 {
            let val = f32::from_le_bytes(data[0..4].try_into().ok()?);
            if val > 0.0 && val < 150.0 {
                return Some(val);
            }
        }
        None
    }
}

impl Drop for SmcConnection {
    fn drop(&mut self) {
        unsafe {
            IOServiceClose(self.conn);
        }
    }
}

// ─── IOHID Temperature Fallback ────────────────────────────────────────────

#[repr(C)]
struct IOHIDServiceClient(c_void);
#[repr(C)]
struct IOHIDEventSystemClient(c_void);
#[repr(C)]
struct IOHIDEvent(c_void);

type IOHIDServiceClientRef = *const IOHIDServiceClient;
type IOHIDEventSystemClientRef = *const IOHIDEventSystemClient;
type IOHIDEventRef = *const IOHIDEvent;

const kHIDPage_AppleVendor: i32 = 0xff00;
const kHIDUsage_AppleVendor_TemperatureSensor: i32 = 0x0005;
const kIOHIDEventTypeTemperature: i64 = 15;

#[link(name = "IOKit", kind = "framework")]
#[rustfmt::skip]
unsafe extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: CFAllocatorRef) -> IOHIDEventSystemClientRef;
    fn IOHIDEventSystemClientSetMatching(a: IOHIDEventSystemClientRef, b: CFDictionaryRef) -> i32;
    fn IOHIDEventSystemClientCopyServices(a: IOHIDEventSystemClientRef) -> CFArrayRef;
    fn IOHIDServiceClientCopyProperty(a: IOHIDServiceClientRef, b: CFStringRef) -> CFStringRef;
    fn IOHIDServiceClientCopyEvent(a: IOHIDServiceClientRef, v0: i64, v1: i32, v2: i64) -> IOHIDEventRef;
    fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: i64) -> f64;
}

fn iohid_gpu_temp() -> Option<f32> {
    let keys = [cfstr("PrimaryUsagePage"), cfstr("PrimaryUsage")];
    let nums = [
        cfnum(kHIDPage_AppleVendor),
        cfnum(kHIDUsage_AppleVendor_TemperatureSensor),
    ];

    let dict = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr() as _,
            nums.as_ptr() as _,
            2,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };

    let result = unsafe {
        let system = IOHIDEventSystemClientCreate(kCFAllocatorDefault);
        if system.is_null() {
            CFRelease(dict as _);
            return None;
        }

        IOHIDEventSystemClientSetMatching(system, dict);
        let services = IOHIDEventSystemClientCopyServices(system);
        if services.is_null() {
            CFRelease(system as _);
            CFRelease(dict as _);
            return None;
        }

        let mut gpu_temps = Vec::new();
        let count = CFArrayGetCount(services);
        for i in 0..count {
            let sc = CFArrayGetValueAtIndex(services, i) as IOHIDServiceClientRef;
            if sc.is_null() {
                continue;
            }
            let product_key = cfstr("Product");
            let name_ref = IOHIDServiceClientCopyProperty(sc, product_key);
            CFRelease(product_key as _);
            if name_ref.is_null() {
                continue;
            }
            let name = from_cfstr(name_ref);
            CFRelease(name_ref as _);

            if !name.starts_with("GPU MTR Temp Sensor") {
                continue;
            }

            let event =
                IOHIDServiceClientCopyEvent(sc, kIOHIDEventTypeTemperature, 0, 0);
            if event.is_null() {
                continue;
            }
            let temp =
                IOHIDEventGetFloatValue(event, kIOHIDEventTypeTemperature << 16) as f32;
            CFRelease(event as _);
            if temp > 0.0 && temp < 150.0 {
                gpu_temps.push(temp);
            }
        }

        CFRelease(services as _);
        CFRelease(system as _);

        if gpu_temps.is_empty() {
            None
        } else {
            Some(gpu_temps.iter().sum::<f32>() / gpu_temps.len() as f32)
        }
    };

    unsafe { CFRelease(dict as _) };
    result
}

// ─── SoC Info ───────────────────────────────────────────────────────────────

fn get_gpu_name() -> String {
    let out = std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output();
    if let Ok(out) = out {
        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            if let Some(name) = json["SPDisplaysDataType"][0]["sppci_model"].as_str() {
                return name.to_string();
            }
        }
    }
    "Apple GPU".to_string()
}

fn get_gpu_freqs() -> Vec<u32> {
    let iter = match IOServiceIterator::new("AppleARMIODevice") {
        Some(it) => it,
        None => return Vec::new(),
    };

    for (entry, name) in iter {
        if name == "pmgr" {
            let mut props: MaybeUninit<CFMutableDictionaryRef> = MaybeUninit::uninit();
            let ok = unsafe {
                IORegistryEntryCreateCFProperties(
                    entry,
                    props.as_mut_ptr(),
                    kCFAllocatorDefault,
                    0,
                )
            };
            if ok != 0 {
                continue;
            }
            let props = unsafe { props.assume_init() };
            let freqs = parse_dvfs_mhz(props, "voltage-states9");
            unsafe { CFRelease(props as _) };
            return freqs;
        }
    }
    Vec::new()
}

fn parse_dvfs_mhz(dict: CFDictionaryRef, key: &str) -> Vec<u32> {
    let obj = match cfdict_get_val(dict, key) {
        Some(o) => o as CFDataRef,
        None => return Vec::new(),
    };
    unsafe {
        let obj_len = CFDataGetLength(obj);
        if obj_len <= 0 {
            return Vec::new();
        }
        let mut obj_val = vec![0u8; obj_len as usize];
        CFDataGetBytes(obj, CFRange::init(0, obj_len), obj_val.as_mut_ptr());

        // Pairs of (freq, voltage), 4 bytes each
        let items_count = (obj_len / 8) as usize;
        let mut freqs = Vec::with_capacity(items_count);
        for chunk in obj_val.chunks_exact(8) {
            let freq = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            freqs.push(freq / (1000 * 1000)); // Hz → MHz
        }
        freqs
    }
}

// ─── AppleGpuMonitor ────────────────────────────────────────────────────────

pub struct AppleGpuMonitor {
    gpu_name: String,
    gpu_freqs: Vec<u32>,
    ior: IOReportSub,
    smc: Option<SmcConnection>,
    smc_gpu_keys: Vec<String>,
    prev_sample: Option<(CFDictionaryRef, std::time::Instant)>,
}

// CFDictionaryRef is a raw pointer — it's Send-safe because we only use it
// within a single monitoring thread and properly manage its lifetime.
unsafe impl Send for AppleGpuMonitor {}

impl AppleGpuMonitor {
    pub fn try_new() -> Option<Self> {
        let channels = vec![
            ("Energy Model", None),
            ("GPU Stats", Some("GPU Performance States")),
        ];
        let ior = IOReportSub::new(&channels)?;

        let gpu_name = get_gpu_name();
        let gpu_freqs = get_gpu_freqs();

        let mut smc = SmcConnection::new();
        let smc_gpu_keys = match smc.as_mut() {
            Some(s) => s.find_gpu_temp_keys(),
            None => Vec::new(),
        };

        Some(Self {
            gpu_name,
            gpu_freqs,
            ior,
            smc,
            smc_gpu_keys,
            prev_sample: None,
        })
    }

    pub fn collect(&mut self) -> Vec<GpuMetrics> {
        let (prev_sample, prev_time) = match self.prev_sample.take() {
            Some((s, t)) => (s, t),
            None => {
                // First call: take a bootstrap sample, wait 100ms, then delta
                let s1 = self.ior.sample();
                std::thread::sleep(std::time::Duration::from_millis(100));
                (s1, std::time::Instant::now() - std::time::Duration::from_millis(100))
            }
        };

        let now = std::time::Instant::now();
        let cur_sample = self.ior.sample();
        let elapsed_ms = now.duration_since(prev_time).as_millis().max(1) as u64;

        let delta = unsafe { IOReportCreateSamplesDelta(prev_sample, cur_sample, null()) };
        unsafe { CFRelease(prev_sample as _) };

        // Store current sample for next call
        self.prev_sample = Some((cur_sample, now));

        if delta.is_null() {
            return vec![self.empty_metrics()];
        }

        let iter = IOReportIterator::new(delta);

        let mut gpu_power: Option<f32> = None;
        let mut gpu_usage: Option<(u32, f32)> = None;

        for x in iter {
            if x.group == "GPU Stats" && x.subgroup == "GPU Performance States" {
                if x.channel == "GPUPH" && !self.gpu_freqs.is_empty() {
                    gpu_usage = Some(self.calc_gpu_freq(x.item));
                }
            }

            if x.group == "Energy Model" && x.channel == "GPU Energy" {
                if let Some(w) = cfio_watts(x.item, &x.unit, elapsed_ms) {
                    gpu_power = Some(gpu_power.unwrap_or(0.0) + w);
                }
            }
        }

        let temperature = self.read_gpu_temp();

        let (utilization, clock_mhz) = match gpu_usage {
            Some((freq, ratio)) => {
                let util = (ratio * 100.0).clamp(0.0, 100.0) as u32;
                Some((util, freq))
            }
            None => None,
        }
        .map_or((None, None), |(u, c)| (Some(u), Some(c)));

        vec![GpuMetrics {
            name: self.gpu_name.clone(),
            utilization,
            temperature,
            memory_total: None,
            memory_used: None,
            clock_mhz,
            power_watts: gpu_power.map(|w| w as f64),
        }]
    }

    fn calc_gpu_freq(&self, item: CFDictionaryRef) -> (u32, f32) {
        let residencies = cfio_get_residencies(item);

        // Skip IDLE/OFF/DOWN states at the beginning
        let offset = residencies
            .iter()
            .position(|x| x.0 != "IDLE" && x.0 != "DOWN" && x.0 != "OFF")
            .unwrap_or(0);

        // Use freqs starting from index 1 (skip first freq entry, matching macmon)
        let freqs = if self.gpu_freqs.len() > 1 {
            &self.gpu_freqs[1..]
        } else {
            &self.gpu_freqs
        };

        let active: f64 = residencies.iter().skip(offset).map(|x| x.1 as f64).sum();
        let total: f64 = residencies.iter().map(|x| x.1 as f64).sum();

        if total == 0.0 || freqs.is_empty() {
            return (0, 0.0);
        }

        let count = freqs.len();
        let mut avg_freq = 0.0f64;
        for i in 0..count {
            if i + offset < residencies.len() {
                let percent = if active > 0.0 {
                    residencies[i + offset].1 as f64 / active
                } else {
                    0.0
                };
                avg_freq += percent * freqs[i] as f64;
            }
        }

        let usage_ratio = if total > 0.0 { active / total } else { 0.0 };
        let min_freq = *freqs.first().unwrap() as f64;
        let max_freq = *freqs.last().unwrap() as f64;

        if max_freq == 0.0 {
            return (0, 0.0);
        }

        let from_max = (avg_freq.max(min_freq) * usage_ratio) / max_freq;
        (avg_freq as u32, from_max as f32)
    }

    fn read_gpu_temp(&mut self) -> Option<u32> {
        // Try SMC first
        if !self.smc_gpu_keys.is_empty() {
            if let Some(smc) = &mut self.smc {
                let mut temps = Vec::new();
                for key in &self.smc_gpu_keys.clone() {
                    if let Some(t) = smc.read_temp(key) {
                        temps.push(t);
                    }
                }
                if !temps.is_empty() {
                    let avg = temps.iter().sum::<f32>() / temps.len() as f32;
                    return Some(avg as u32);
                }
            }
        }

        // Fallback to IOHID
        iohid_gpu_temp().map(|t| t as u32)
    }

    fn empty_metrics(&self) -> GpuMetrics {
        GpuMetrics {
            name: self.gpu_name.clone(),
            utilization: None,
            temperature: None,
            memory_total: None,
            memory_used: None,
            clock_mhz: None,
            power_watts: None,
        }
    }
}

impl Drop for AppleGpuMonitor {
    fn drop(&mut self) {
        if let Some((sample, _)) = self.prev_sample.take() {
            unsafe { CFRelease(sample as _) };
        }
    }
}
