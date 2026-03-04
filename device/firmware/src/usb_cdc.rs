#![allow(static_mut_refs)]

use core::ptr::addr_of;
use core::sync::atomic::{compiler_fence, Ordering};

// Endpoint sizes
const EP0_SIZE: usize = 64;
const EP1_SIZE: usize = 8;
const EP2_SIZE: usize = 64;
const EP3_SIZE: usize = 64;

// USB PID tokens (from int_st bits [5:4], matching WCH SDK / ch32-metapac UsbToken)
const PID_OUT: u8 = 0b00;   // 0
const PID_IN: u8 = 0b10;    // 2
const PID_SETUP: u8 = 0b11; // 3

// USB standard request codes
const USB_REQ_GET_STATUS: u8 = 0;
const USB_REQ_CLEAR_FEATURE: u8 = 1;
const USB_REQ_SET_ADDRESS: u8 = 5;
const USB_REQ_GET_DESCRIPTOR: u8 = 6;
const USB_REQ_SET_CONFIGURATION: u8 = 9;

// USB descriptor types
const DESC_DEVICE: u8 = 1;
const DESC_CONFIGURATION: u8 = 2;
const DESC_STRING: u8 = 3;

// CDC class request codes
const CDC_SET_LINE_CODING: u8 = 0x20;
const CDC_GET_LINE_CODING: u8 = 0x21;
const CDC_SET_CONTROL_LINE_STATE: u8 = 0x22;

// USB request type masks
const USB_REQ_TYPE_STANDARD: u8 = 0x00;
const USB_REQ_TYPE_CLASS: u8 = 0x20;
const USB_REQ_TYPE_MASK: u8 = 0x60;

// DMA buffers - must be aligned and in SRAM
#[repr(align(4))]
struct AlignedBuf<const N: usize>([u8; N]);

static mut EP0_BUF: AlignedBuf<EP0_SIZE> = AlignedBuf([0u8; EP0_SIZE]);
static mut EP1_BUF: AlignedBuf<EP1_SIZE> = AlignedBuf([0u8; EP1_SIZE]);
static mut EP2_BUF: AlignedBuf<EP2_SIZE> = AlignedBuf([0u8; EP2_SIZE]);
static mut EP3_BUF: AlignedBuf<EP3_SIZE> = AlignedBuf([0u8; EP3_SIZE]);

// EP0 control transfer state
static mut USB_CONFIG: u8 = 0;
static mut USB_ADDRESS: u8 = 0;
static mut SETUP_REQ_CODE: u8 = 0;
static mut EP0_TX_DATA: &[u8] = &[];

// CDC line coding: dwDTERate(4 LE), bCharFormat(1), bParityType(1), bDataBits(1)
// Default: 115200 baud, 1 stop bit, no parity, 8 data bits
static mut LINE_CODING: [u8; 7] = [0x00, 0xC2, 0x01, 0x00, 0x00, 0x00, 0x08];

// Device Descriptor (VID=0x0200, PID=0x02DB)
static DEV_DESC: [u8; 18] = [
    0x12, 0x01, // bLength, bDescriptorType
    0x10, 0x01, // bcdUSB 1.10
    0x02, 0x02, 0x00, // bDeviceClass=CDC, SubClass, Protocol
    0x40, // bMaxPacketSize0 = 64
    0x00, 0x02, // idVendor (0x0200 LE)
    0xDB, 0x02, // idProduct (0x02DB LE)
    0x00, 0x01, // bcdDevice v1.0
    0x01, 0x02, 0x03, // iManufacturer, iProduct, iSerialNumber
    0x01, // bNumConfigurations
];

// Configuration Descriptor (67 bytes total)
static CFG_DESC: [u8; 67] = [
    // Configuration Descriptor
    0x09, 0x02,
    67, 0x00, // wTotalLength
    0x02, // bNumInterfaces
    0x01, // bConfigurationValue
    0x00, // iConfiguration
    0x80, // bmAttributes (bus powered)
    0x32, // bMaxPower (100mA)
    // Interface 0: CDC Communication
    0x09, 0x04,
    0x00, 0x00, 0x01, // bInterfaceNumber, bAlternateSetting, bNumEndpoints
    0x02, 0x02, 0x01, // Class=CDC, SubClass=ACM, Protocol=AT
    0x00,
    // CDC Header Functional Descriptor
    0x05, 0x24, 0x00, 0x10, 0x01,
    // CDC Call Management Functional Descriptor
    0x05, 0x24, 0x01, 0x00, 0x01,
    // CDC ACM Functional Descriptor
    0x04, 0x24, 0x02, 0x02,
    // CDC Union Functional Descriptor
    0x05, 0x24, 0x06, 0x00, 0x01,
    // EP1 IN - CDC Notification (Interrupt)
    0x07, 0x05,
    0x81, // bEndpointAddress (EP1 IN)
    0x03, // bmAttributes (Interrupt)
    0x08, 0x00, // wMaxPacketSize
    0xFF, // bInterval
    // Interface 1: CDC Data
    0x09, 0x04,
    0x01, 0x00, 0x02, // bInterfaceNumber, bAlternateSetting, bNumEndpoints
    0x0A, 0x00, 0x00, // Class=CDC Data, SubClass, Protocol
    0x00,
    // EP2 OUT - Bulk Data
    0x07, 0x05,
    0x02, // bEndpointAddress (EP2 OUT)
    0x02, // bmAttributes (Bulk)
    0x40, 0x00, // wMaxPacketSize (64)
    0x00,
    // EP3 IN - Bulk Data
    0x07, 0x05,
    0x83, // bEndpointAddress (EP3 IN)
    0x02, // bmAttributes (Bulk)
    0x40, 0x00, // wMaxPacketSize (64)
    0x00,
];

// String Descriptors
static STR_DESC_0: [u8; 4] = [0x04, 0x03, 0x09, 0x04]; // Language ID (English US)
static STR_DESC_1: [u8; 18] = [
    18, 0x03,
    b'p', 0, b'o', 0, b'i', 0, b'n', 0,
    b't', 0, b'i', 0, b'f', 0, b'y', 0,
];
static STR_DESC_2: [u8; 18] = [
    18, 0x03,
    b'P', 0, b'o', 0, b'i', 0, b'n', 0,
    b't', 0, b'i', 0, b'f', 0, b'y', 0,
];
static STR_DESC_3: [u8; 10] = [
    10, 0x03,
    b'0', 0, b'0', 0, b'0', 0, b'1', 0,
];

// USBFS register base and offsets (all 8-bit except DMA which is 32-bit)
const USB: usize = 0x4002_3400;
const R_BASE_CTRL: usize = USB;           // 0x00
const R_UDEV_CTRL: usize = USB + 0x01;
const R_INT_EN: usize    = USB + 0x02;
const R_DEV_ADDR: usize  = USB + 0x03;
const R_INT_FG: usize    = USB + 0x06;
const R_INT_ST: usize    = USB + 0x07;
const R_RX_LEN: usize    = USB + 0x08;
const R_UEP4_1_MOD: usize = USB + 0x0C;
const R_UEP2_3_MOD: usize = USB + 0x0D;
const R_UEP0_DMA: usize  = USB + 0x10;    // 32-bit
const R_UEP1_DMA: usize  = USB + 0x14;
const R_UEP2_DMA: usize  = USB + 0x18;
const R_UEP3_DMA: usize  = USB + 0x1C;
// Per-endpoint: TX_LEN at +0, TX_CTRL at +1, CTRL_H at +2
const R_UEP0_TX_LEN: usize = USB + 0x20;
const R_UEP0_CTRL_H: usize = USB + 0x22;
const R_UEP1_TX_LEN: usize = USB + 0x24;
const R_UEP1_CTRL_H: usize = USB + 0x26;
const R_UEP2_CTRL_H: usize = USB + 0x2A;
const R_UEP3_TX_LEN: usize = USB + 0x2C;
const R_UEP3_CTRL_H: usize = USB + 0x2E;

// CTRL_H bit definitions (matching WCH SDK)
const UEP_T_RES_ACK: u8   = 0x00;
const UEP_T_RES_NAK: u8   = 0x02;
const UEP_T_RES_STALL: u8 = 0x03;
const UEP_T_RES_MASK: u8  = 0x03;
const UEP_R_RES_ACK: u8   = 0x00;
const UEP_R_RES_NAK: u8   = 0x08;
const UEP_R_RES_STALL: u8 = 0x0C;
const UEP_T_TOG: u8       = 0x40;
const UEP_R_TOG: u8       = 0x80;

#[inline(always)]
unsafe fn w8(addr: usize, val: u8) {
    (addr as *mut u8).write_volatile(val);
}

#[inline(always)]
unsafe fn r8(addr: usize) -> u8 {
    (addr as *const u8).read_volatile()
}

#[inline(always)]
unsafe fn w32(addr: usize, val: u32) {
    (addr as *mut u32).write_volatile(val);
}

pub fn usb_init() {
    // Enable required clocks: AFIO (APB2 bit 0), GPIOC (APB2 bit 4), USBFS (AHB bit 12)
    unsafe {
        let rcc_apb2 = 0x4002_1018 as *mut u32;
        let val = rcc_apb2.read_volatile();
        rcc_apb2.write_volatile(val | (1 << 0) | (1 << 4));

        let rcc_ahb = 0x4002_1014 as *mut u32;
        let val = rcc_ahb.read_volatile();
        rcc_ahb.write_volatile(val | (1 << 12));
    }

    // GPIO: PC16=floating input (D-), PC17=floating input (D+)
    // USB PHY controls these pins via AFIO USB_IOEN
    unsafe {
        let cfgxr = 0x4001_101C as *mut u32;
        let val = cfgxr.read_volatile();
        cfgxr.write_volatile((val & !0xFF) | 0x44);
    }

    // AFIO CTLR: USB_IOEN | USB_PHY_V33 | UDP_PUE_1K5 = 0xCC
    unsafe {
        let afio_ctlr = 0x4001_0018 as *mut u32;
        let val = afio_ctlr.read_volatile();
        afio_ctlr.write_volatile((val & !0xFF) | 0xCC);
    }

    unsafe {
        // BASE_CTRL = 0 (clear before config, no RESET_SIE)
        w8(R_BASE_CTRL, 0x00);

        // --- Endpoint init ---

        // UEP4_1_MOD: EP1 TX enable = bit 6 (0x40)
        w8(R_UEP4_1_MOD, 0x40);
        // UEP2_3_MOD: EP2 RX enable (bit 3) | EP3 TX enable (bit 6) = 0x48
        w8(R_UEP2_3_MOD, 0x08 | 0x40);

        // DMA buffer addresses
        w32(R_UEP0_DMA, addr_of!(EP0_BUF) as u32);
        w32(R_UEP1_DMA, addr_of!(EP1_BUF) as u32);
        w32(R_UEP2_DMA, addr_of!(EP2_BUF) as u32);
        w32(R_UEP3_DMA, addr_of!(EP3_BUF) as u32);

        // EP0: CTRL_H = R_RES_ACK | T_RES_NAK
        w8(R_UEP0_CTRL_H, UEP_R_RES_ACK | UEP_T_RES_NAK);
        // EP2: CTRL_H = R_RES_ACK
        w8(R_UEP2_CTRL_H, UEP_R_RES_ACK);

        // EP1: TX_LEN=0, CTRL_H = T_RES_NAK
        w8(R_UEP1_TX_LEN, 0);
        w8(R_UEP1_CTRL_H, UEP_T_RES_NAK);
        // EP3: TX_LEN=0, CTRL_H = T_RES_NAK
        w8(R_UEP3_TX_LEN, 0);
        w8(R_UEP3_CTRL_H, UEP_T_RES_NAK);

        // --- End endpoint init ---

        // DEV_ADDR = 0
        w8(R_DEV_ADDR, 0x00);
        // BASE_CTRL = UC_DEV_PU_EN(0x20) | UC_INT_BUSY(0x08) | UC_DMA_EN(0x01) = 0x29
        w8(R_BASE_CTRL, 0x29);
        // INT_FG = 0xFF (clear all flags)
        w8(R_INT_FG, 0xFF);
        // UDEV_CTRL = UD_PD_DIS(0x80) | UD_PORT_EN(0x01) = 0x81
        w8(R_UDEV_CTRL, 0x81);
        // INT_EN = UIE_SUSPEND(0x04) | UIE_BUS_RST(0x01) | UIE_TRANSFER(0x02) = 0x07
        w8(R_INT_EN, 0x07);
    }

    unsafe {
        USB_CONFIG = 0;
        USB_ADDRESS = 0;
    }
}

/// Poll USB for events. Returns Some(len) when bulk data received on EP2.
pub fn usb_poll(rx_buf: &mut [u8]) -> Option<usize> {
    let flags = unsafe { r8(R_INT_FG) };

    if flags & 0x01 != 0 {
        // Bus reset - also clear transfer flag to avoid stale INT_ST
        handle_bus_reset();
        unsafe { w8(R_INT_FG, 0x03); }
        return None;
    }

    if flags & 0x02 != 0 {
        // Transfer complete
        let int_st = unsafe { r8(R_INT_ST) };
        let ep = int_st & 0x0F;
        let token = (int_st >> 4) & 0x03;

        // CH32X033 reports unexpected ep values in INT_ST (e.g. ep=2 for EP0 SETUP).
        // SETUP always targets EP0. For OUT, use ep field + SETUP_REQ_CODE to disambiguate.
        let result = match token {
            PID_SETUP => { handle_ep0_setup(); None }
            PID_IN => { handle_ep0_in(); None }
            PID_OUT => {
                if unsafe { SETUP_REQ_CODE == CDC_SET_LINE_CODING } || ep == 0 {
                    handle_ep0_out();
                    None
                } else {
                    handle_ep2_out(rx_buf)
                }
            }
            _ => None,
        };

        unsafe { w8(R_INT_FG, 0x02); }
        return result;
    }

    if flags & 0x04 != 0 {
        // Suspend
        unsafe { w8(R_INT_FG, 0x04); }
    }

    None
}

#[allow(dead_code)]
pub fn is_configured() -> bool {
    unsafe { USB_CONFIG != 0 }
}

fn handle_bus_reset() {
    unsafe {
        USB_CONFIG = 0;
        USB_ADDRESS = 0;
        EP0_TX_DATA = &[];

        w8(R_DEV_ADDR, 0x00);

        // Re-init endpoints
        w8(R_UEP4_1_MOD, 0x40);
        w8(R_UEP2_3_MOD, 0x08 | 0x40);

        w32(R_UEP0_DMA, addr_of!(EP0_BUF) as u32);
        w32(R_UEP1_DMA, addr_of!(EP1_BUF) as u32);
        w32(R_UEP2_DMA, addr_of!(EP2_BUF) as u32);
        w32(R_UEP3_DMA, addr_of!(EP3_BUF) as u32);

        w8(R_UEP0_CTRL_H, UEP_R_RES_ACK | UEP_T_RES_NAK);
        w8(R_UEP2_CTRL_H, UEP_R_RES_ACK);
        w8(R_UEP1_TX_LEN, 0);
        w8(R_UEP1_CTRL_H, UEP_T_RES_NAK);
        w8(R_UEP3_TX_LEN, 0);
        w8(R_UEP3_CTRL_H, UEP_T_RES_NAK);
    }
}

fn handle_ep0_setup() {
    // USB SETUP packets are always 8 bytes by spec.
    // CH32X033 RX_LEN register returns incorrect values for SETUP, so skip check.

    // NAK both directions while processing (matching SDK)
    unsafe {
        w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_NAK | UEP_R_TOG | UEP_R_RES_NAK);
    }

    compiler_fence(Ordering::SeqCst);
    let setup = unsafe { &EP0_BUF.0[..8] };
    let bm_request_type = setup[0];
    let b_request = setup[1];
    let w_value = u16::from_le_bytes([setup[2], setup[3]]);
    let w_length = u16::from_le_bytes([setup[6], setup[7]]);

    unsafe {
        SETUP_REQ_CODE = b_request;
        EP0_TX_DATA = &[];
    }

    match bm_request_type & USB_REQ_TYPE_MASK {
        USB_REQ_TYPE_STANDARD => {
            handle_standard_request(bm_request_type, b_request, w_value, w_length)
        }
        USB_REQ_TYPE_CLASS => handle_class_request(b_request, w_length),
        _ => stall_ep0(),
    }
}

fn handle_standard_request(bm_request_type: u8, b_request: u8, w_value: u16, w_length: u16) {
    match b_request {
        USB_REQ_GET_STATUS => {
            unsafe {
                EP0_BUF.0[0] = 0;
                EP0_BUF.0[1] = 0;
                compiler_fence(Ordering::SeqCst);
                let len = if w_length < 2 { w_length as u8 } else { 2 };
                w8(R_UEP0_TX_LEN, len);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        USB_REQ_CLEAR_FEATURE => {
            unsafe {
                w8(R_UEP0_TX_LEN, 0);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        USB_REQ_SET_ADDRESS => {
            unsafe {
                USB_ADDRESS = (w_value & 0x7F) as u8;
                w8(R_UEP0_TX_LEN, 0);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        USB_REQ_GET_DESCRIPTOR => {
            let desc_type = (w_value >> 8) as u8;
            let desc_index = (w_value & 0xFF) as u8;

            let desc: &'static [u8] = match desc_type {
                DESC_DEVICE => &DEV_DESC,
                DESC_CONFIGURATION => &CFG_DESC,
                DESC_STRING => match desc_index {
                    0 => &STR_DESC_0,
                    1 => &STR_DESC_1,
                    2 => &STR_DESC_2,
                    3 => &STR_DESC_3,
                    _ => { stall_ep0(); return; }
                },
                _ => { stall_ep0(); return; }
            };

            let send_len = (w_length as usize).min(desc.len());
            let chunk = send_len.min(EP0_SIZE);

            unsafe {
                EP0_BUF.0[..chunk].copy_from_slice(&desc[..chunk]);
                EP0_TX_DATA = if send_len > chunk {
                    &desc[chunk..send_len]
                } else {
                    &[]
                };
                compiler_fence(Ordering::SeqCst);
                w8(R_UEP0_TX_LEN, chunk as u8);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        USB_REQ_SET_CONFIGURATION => {
            unsafe {
                USB_CONFIG = (w_value & 0xFF) as u8;
                w8(R_UEP0_TX_LEN, 0);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        _ => {
            if bm_request_type & 0x80 != 0 {
                unsafe {
                    w8(R_UEP0_TX_LEN, 0);
                    w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
                }
            } else {
                stall_ep0();
            }
        }
    }
}

fn handle_class_request(b_request: u8, w_length: u16) {
    match b_request {
        CDC_GET_LINE_CODING => {
            let len = if w_length < 7 { w_length as u8 } else { 7 };
            unsafe {
                EP0_BUF.0[..7].copy_from_slice(&LINE_CODING);
                compiler_fence(Ordering::SeqCst);
                w8(R_UEP0_TX_LEN, len);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        CDC_SET_LINE_CODING => {
            // Data arrives in EP0 OUT phase
            unsafe {
                w8(R_UEP0_CTRL_H, UEP_R_TOG | UEP_R_RES_ACK);
            }
        }

        CDC_SET_CONTROL_LINE_STATE => {
            unsafe {
                w8(R_UEP0_TX_LEN, 0);
                w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            }
        }

        _ => stall_ep0(),
    }
}

fn handle_ep0_in() {
    unsafe {
        // Apply deferred SET_ADDRESS
        if SETUP_REQ_CODE == USB_REQ_SET_ADDRESS {
            let addr = USB_ADDRESS & 0x7F;
            w8(R_DEV_ADDR, (r8(R_DEV_ADDR) & 0x80) | addr);
            SETUP_REQ_CODE = 0;
        }

        // Send remaining descriptor data
        if !EP0_TX_DATA.is_empty() {
            let chunk = EP0_TX_DATA.len().min(EP0_SIZE);
            EP0_BUF.0[..chunk].copy_from_slice(&EP0_TX_DATA[..chunk]);
            EP0_TX_DATA = &EP0_TX_DATA[chunk..];
            compiler_fence(Ordering::SeqCst);
            w8(R_UEP0_TX_LEN, chunk as u8);
            // Toggle T_TOG
            let ctrl = r8(R_UEP0_CTRL_H);
            w8(R_UEP0_CTRL_H, (ctrl ^ UEP_T_TOG) & !(UEP_T_RES_MASK) | UEP_T_RES_ACK);
        } else {
            // Transfer complete - accept status OUT
            w8(R_UEP0_TX_LEN, 0);
            w8(R_UEP0_CTRL_H, UEP_R_TOG | UEP_R_RES_ACK);
        }
    }
}

fn handle_ep0_out() {
    unsafe {
        if SETUP_REQ_CODE == CDC_SET_LINE_CODING {
            let len = r8(R_RX_LEN) as usize;
            compiler_fence(Ordering::SeqCst);
            if len == 7 {
                LINE_CODING.copy_from_slice(&EP0_BUF.0[..7]);
            }
            w8(R_UEP0_TX_LEN, 0);
            w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_ACK);
            SETUP_REQ_CODE = 0;
        } else {
            // Status stage OUT complete
            w8(R_UEP0_TX_LEN, 0);
            w8(R_UEP0_CTRL_H, UEP_T_RES_NAK | UEP_R_RES_ACK);
        }
    }
}

fn handle_ep2_out(rx_buf: &mut [u8]) -> Option<usize> {
    let len = unsafe { r8(R_RX_LEN) } as usize;

    if len > 0 && len <= rx_buf.len() {
        compiler_fence(Ordering::SeqCst);
        unsafe {
            rx_buf[..len].copy_from_slice(&EP2_BUF.0[..len]);
            w8(R_UEP2_CTRL_H, UEP_R_RES_ACK);
        }
        Some(len)
    } else {
        None
    }
}

fn stall_ep0() {
    unsafe {
        w8(R_UEP0_TX_LEN, 0);
        w8(R_UEP0_CTRL_H, UEP_T_TOG | UEP_T_RES_STALL | UEP_R_TOG | UEP_R_RES_STALL);
    }
}

// Bootloader entry register addresses
const FLASH_STATR: usize = 0x4002_200C;
const FLASH_BOOT_MODEKEYR: usize = 0x4002_2028;
const RCC_RSTSCKR: usize = 0x4002_1024;
const PFIC_CFGR: usize = 0xE000_E048;

/// Enter USB bootloader mode via software reset sequence.
/// Unlocks BOOT_MODE, sets it, clears reset flags, then triggers software reset.
pub fn enter_bootloader() -> ! {
    unsafe {
        // Disable all interrupts
        core::arch::asm!("csrci mstatus, 8");

        compiler_fence(Ordering::SeqCst);

        // 1. Unlock BOOT_MODE by writing key sequence to FLASH_BOOT_MODEKEYR
        w32(FLASH_BOOT_MODEKEYR, 0x45670123);
        w32(FLASH_BOOT_MODEKEYR, 0xCDEF89AB);

        compiler_fence(Ordering::SeqCst);

        // 2. Set BOOT_MODE (bit 14) in FLASH_STATR
        let val = (FLASH_STATR as *const u32).read_volatile();
        w32(FLASH_STATR, val | (1 << 14));

        compiler_fence(Ordering::SeqCst);

        // 3. Clear all reset flags: set RMVF (bit 24) in RCC_RSTSCKR
        let val = (RCC_RSTSCKR as *const u32).read_volatile();
        w32(RCC_RSTSCKR, val | (1 << 24));

        compiler_fence(Ordering::SeqCst);

        // 4. Trigger software reset via PFIC_CFGR
        w32(PFIC_CFGR, 0xBEEF0080);
    }

    loop {}
}
