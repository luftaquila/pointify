#![allow(static_mut_refs)]

use core::ptr::addr_of;

use ch32_hal::pac;

// Endpoint sizes
const EP0_SIZE: usize = 64;
const EP1_SIZE: usize = 8;
const EP2_SIZE: usize = 64;
const EP3_SIZE: usize = 64;

// USB PID tokens (from int_st.mask_token)
const PID_SETUP: u8 = 0b00;
const PID_IN: u8 = 0b01;
const PID_OUT: u8 = 0b10;

// Endpoint response types (for t_res / r_res fields)
const RES_ACK: u8 = 0;
const RES_NAK: u8 = 2;
const RES_STALL: u8 = 3;

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
    0x00, 0x01, // bcdDevice 1.00
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

fn usbd() -> pac::usb::Usbd {
    unsafe { pac::usb::Usbd::from_ptr(pac::USBFS.as_ptr()) }
}

pub fn usb_init() {
    let usb = usbd();

    // Enable USBFS clock
    pac::RCC.ahbpcenr().modify(|w| w.set_usbfsen(true));

    // Reset USB SIE
    usb.ctrl().write(|w| {
        w.set_reset_sie(true);
        w.set_clr_all(true);
    });
    for _ in 0..100 {
        unsafe { core::arch::asm!("nop") };
    }
    usb.ctrl().write(|w| {
        w.set_reset_sie(false);
        w.set_clr_all(false);
    });

    // Set device address to 0
    usb.dev_ad().write(|w| w.set_mask_usb_addr(0));

    // Configure EP DMA buffer addresses
    usb.uep0123_dma(0)
        .write_value(pac::usb::regs::UepDma(addr_of!(EP0_BUF) as u32));
    usb.uep0123_dma(1)
        .write_value(pac::usb::regs::UepDma(addr_of!(EP1_BUF) as u32));
    usb.uep0123_dma(2)
        .write_value(pac::usb::regs::UepDma(addr_of!(EP2_BUF) as u32));
    usb.uep0123_dma(3)
        .write_value(pac::usb::regs::UepDma(addr_of!(EP3_BUF) as u32));

    // EP4/1 mode: EP1 TX enabled (CDC notification IN)
    usb.uep4_1_mod().write(|w| {
        w.set_tx_en(0, true); // EP1 TX
    });

    // EP2/3 mode: EP2 RX enabled (bulk OUT), EP3 TX enabled (bulk IN)
    usb.uep2_3_mod().write(|w| {
        w.set_rx_en(0, true); // EP2 RX
        w.set_tx_en(1, true); // EP3 TX
    });

    // EP0: NAK TX, ACK RX
    usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(0).write(|w| {
        w.set_t_res(RES_NAK);
        w.set_r_res(RES_ACK);
    });

    // EP1: NAK TX, auto-toggle (bit 4 = T_AUTO_TOG)
    usb.uep01234_t_len(1).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(1)
        .write_value(pac::usb::regs::UepCtrl(RES_NAK | (1 << 4)));

    // EP2: ACK RX, auto-toggle (bit 5 = R_AUTO_TOG)
    usb.uep01234_t_len(2).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(2)
        .write_value(pac::usb::regs::UepCtrl((RES_ACK << 2) | (1 << 5)));

    // EP3: NAK TX, auto-toggle (bit 4 = T_AUTO_TOG)
    usb.uep01234_t_len(3).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(3)
        .write_value(pac::usb::regs::UepCtrl(RES_NAK | (1 << 4)));

    // Enable interrupts: bus reset, transfer, suspend
    usb.int_en().write(|w| {
        w.set_bus_rst(true);
        w.set_transfer(true);
        w.set_suspend(true);
    });

    // Enable device mode with DMA + internal pull-up + auto-busy
    usb.ctrl().write(|w| {
        w.set_dma_en(true);
        w.set_int_busy(true);
        w.set_sys_ctrl(0b11); // device enabled + pull-up
    });

    // Enable device port, disable pull-down
    usb.udev_ctrl().write(|w| {
        w.set_port_en(true);
        w.set_pd_dis(true);
    });

    unsafe {
        USB_CONFIG = 0;
        USB_ADDRESS = 0;
    }
}

/// Poll USB for events. Returns Some(len) when bulk data received on EP2.
pub fn usb_poll(rx_buf: &mut [u8]) -> Option<usize> {
    let usb = usbd();
    let int_fg = usb.int_fg().read();

    if int_fg.bus_rst() {
        handle_bus_reset();
        usb.int_fg().write(|w| w.set_bus_rst(true));
        return None;
    }

    if int_fg.transfer() {
        let int_st = usb.int_st().read();
        let ep = int_st.mask_uis_endp();
        let token = int_st.mask_token();

        let result = match (ep, token) {
            (0, PID_SETUP) => {
                handle_ep0_setup();
                None
            }
            (0, PID_IN) => {
                handle_ep0_in();
                None
            }
            (0, PID_OUT) => {
                handle_ep0_out();
                None
            }
            (2, PID_OUT) => handle_ep2_out(rx_buf),
            _ => None,
        };

        usb.int_fg().write(|w| w.set_transfer(true));
        return result;
    }

    if int_fg.suspend() {
        usb.int_fg().write(|w| w.set_suspend(true));
    }

    None
}

#[allow(dead_code)]
pub fn is_configured() -> bool {
    unsafe { USB_CONFIG != 0 }
}

fn handle_bus_reset() {
    let usb = usbd();

    unsafe {
        USB_CONFIG = 0;
        USB_ADDRESS = 0;
        EP0_TX_DATA = &[];
    }

    usb.dev_ad().write(|w| w.set_mask_usb_addr(0));

    // Reset EP0
    usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(0).write(|w| {
        w.set_t_res(RES_NAK);
        w.set_r_res(RES_ACK);
    });

    // Reset EP1
    usb.uep01234_t_len(1).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(1)
        .write_value(pac::usb::regs::UepCtrl(RES_NAK | (1 << 4)));

    // Reset EP2
    usb.uep01234_ctrl(2)
        .write_value(pac::usb::regs::UepCtrl((RES_ACK << 2) | (1 << 5)));

    // Reset EP3
    usb.uep01234_t_len(3).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(3)
        .write_value(pac::usb::regs::UepCtrl(RES_NAK | (1 << 4)));
}

fn handle_ep0_setup() {
    let usb = usbd();
    let len = usb.rx_len().read().rx_len() as usize;

    if len != 8 {
        stall_ep0();
        return;
    }

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
    let usb = usbd();

    match b_request {
        USB_REQ_GET_STATUS => {
            unsafe {
                EP0_BUF.0[0] = 0;
                EP0_BUF.0[1] = 0;
            }
            let len = w_length.min(2) as u8;
            usb.uep01234_t_len(0).write(|w| w.set_t_len(len));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
        }

        USB_REQ_CLEAR_FEATURE => {
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
        }

        USB_REQ_SET_ADDRESS => {
            unsafe {
                USB_ADDRESS = (w_value & 0x7F) as u8;
            }
            // ZLP status stage; address applied in handle_ep0_in
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
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
                    _ => {
                        stall_ep0();
                        return;
                    }
                },
                _ => {
                    stall_ep0();
                    return;
                }
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
            }

            usb.uep01234_t_len(0).write(|w| w.set_t_len(chunk as u8));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true); // DATA1
            });
        }

        USB_REQ_SET_CONFIGURATION => {
            unsafe {
                USB_CONFIG = (w_value & 0xFF) as u8;
            }
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
        }

        _ => {
            if bm_request_type & 0x80 != 0 {
                // Unknown IN request - send ZLP
                usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
                usb.uep01234_ctrl(0).write(|w| {
                    w.set_t_res(RES_ACK);
                    w.set_t_tog(true);
                });
            } else {
                stall_ep0();
            }
        }
    }
}

fn handle_class_request(b_request: u8, w_length: u16) {
    let usb = usbd();

    match b_request {
        CDC_GET_LINE_CODING => {
            let len = w_length.min(7) as u8;
            unsafe {
                EP0_BUF.0[..7].copy_from_slice(&LINE_CODING);
            }
            usb.uep01234_t_len(0).write(|w| w.set_t_len(len));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
        }

        CDC_SET_LINE_CODING => {
            // Data arrives in EP0 OUT phase; prepare to receive
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_r_res(RES_ACK);
            });
        }

        CDC_SET_CONTROL_LINE_STATE => {
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
        }

        _ => stall_ep0(),
    }
}

fn handle_ep0_in() {
    let usb = usbd();

    unsafe {
        // Apply deferred SET_ADDRESS
        if SETUP_REQ_CODE == USB_REQ_SET_ADDRESS {
            usb.dev_ad().write(|w| w.set_mask_usb_addr(USB_ADDRESS));
            SETUP_REQ_CODE = 0;
        }

        // Send remaining descriptor data
        if !EP0_TX_DATA.is_empty() {
            let chunk = EP0_TX_DATA.len().min(EP0_SIZE);
            EP0_BUF.0[..chunk].copy_from_slice(&EP0_TX_DATA[..chunk]);
            EP0_TX_DATA = &EP0_TX_DATA[chunk..];

            usb.uep01234_t_len(0).write(|w| w.set_t_len(chunk as u8));
            usb.uep01234_ctrl(0).modify(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(!w.t_tog());
            });
        } else {
            // Transfer complete, reset EP0
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_NAK);
                w.set_r_res(RES_ACK);
            });
        }
    }
}

fn handle_ep0_out() {
    let usb = usbd();

    unsafe {
        if SETUP_REQ_CODE == CDC_SET_LINE_CODING {
            // Receive line coding data
            let len = usb.rx_len().read().rx_len() as usize;
            if len == 7 {
                LINE_CODING.copy_from_slice(&EP0_BUF.0[..7]);
            }
            // Send status IN ZLP
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_ACK);
                w.set_t_tog(true);
            });
            SETUP_REQ_CODE = 0;
        } else {
            // Status stage OUT complete, reset EP0
            usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
            usb.uep01234_ctrl(0).write(|w| {
                w.set_t_res(RES_NAK);
                w.set_r_res(RES_ACK);
            });
        }
    }
}

fn handle_ep2_out(rx_buf: &mut [u8]) -> Option<usize> {
    let usb = usbd();
    let len = usb.rx_len().read().rx_len() as usize;

    if len > 0 && len <= rx_buf.len() {
        unsafe {
            rx_buf[..len].copy_from_slice(&EP2_BUF.0[..len]);
        }

        // Ready for next packet
        usb.uep01234_ctrl(2)
            .write_value(pac::usb::regs::UepCtrl((RES_ACK << 2) | (1 << 5)));

        Some(len)
    } else {
        None
    }
}

fn stall_ep0() {
    let usb = usbd();
    usb.uep01234_t_len(0).write(|w| w.set_t_len(0));
    usb.uep01234_ctrl(0).write(|w| {
        w.set_t_res(RES_STALL);
        w.set_r_res(RES_STALL);
    });
}
