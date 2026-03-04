# CLAUDE.md

## What this is

Pointify is a retro analog gauge meter system for hardware monitoring. A Tauri desktop app reads system metrics and sends PWM values over USB serial to a CH32X033F8P6 board that drives the gauges.

## Repo layout

- `gui/` - Tauri 2 desktop app (Rust backend + HTML frontend)
  - `gui/src-tauri/src/lib.rs` - main app: serial port, config, tray icon, Tauri commands
  - `gui/src-tauri/src/monitor/` - system metrics (CPU, GPU, memory, disk, network)
  - `gui/src-tauri/src/claude.rs` - Claude API usage tracking
  - `gui/src/` - frontend HTML
- `device/firmware/` - CH32X033F8P6 MCU firmware (Rust, no_std)
  - `device/firmware/src/main.rs` - PWM init + main polling loop
  - `device/firmware/src/usb_cdc.rs` - raw PAC register USB CDC ACM driver
- `device/hardware/` - KiCad PCB design files and gerber exports

## How to build

GUI app:
```bash
cd gui && npm install && npm run tauri dev    # dev
cd gui && npm run tauri build                 # production
```

Firmware:
```bash
cd device/firmware && cargo build --release   # build only
cd device/firmware && cargo run --release     # build + flash via probe-rs
```

## Technical notes

### Serial protocol (GUI -> device)
- 115200 baud, USB CDC ACM, VID=0x0200 PID=0x02DB
- Each value: u16 big-endian. Bit 15 = voltage flag (0=3V, 1=5V), bits 14-10 = gauge index (0-9), bits 9-0 = PWM value (0-1023)
- GUI filters serial ports by VID/PID match (VID=512, PID=731 in decimal)

### Firmware
- ch32-hal has no USB driver for CH32X033 USBFS, so USB CDC is done with raw PAC register access (`pac::usb::Usbd`)
- Single embassy main task polls USB and updates PWM in a tight loop. No interrupts, no async timers.
- Reads internal VrefInt (ADC channel 15) at startup to figure out if it's on 3.3V or 5V
- On 5V supply with a 3V gauge, duty gets scaled by 3/5

### Firmware versioning
- Version is stored in the USB device descriptor `bcdDevice` field (`usb_cdc.rs` DEV_DESC bytes 12-13, little-endian)
- BCD-encoded: high byte = major, low byte = minor (e.g. `0x00, 0x01` LE → `0x0100` → v1.0)
- GUI reads it via `nusb::list_devices()` → `DeviceInfo::device_version()` and displays as `vMAJOR.MINOR` next to the Device sidebar header
- Release workflow parses the same bytes to name the artifact `pointify-firmware-vX.Y.elf`

### PWM timer remap constraints
CH32X033F8P6 (20-pin) shares one AFIO remap value per timer. Only 7 of the 10 PCB channels actually work at the same time:
- TIM2 remap=0: PA0(CH1), PA1(CH2), PA2(CH3), PA3(CH4) - 4 channels
- TIM3 remap=2: PC19(CH1) - 1 channel
- TIM1 remap=1: PA7(CH1N), PB1(CH3N) - 2 channels (ComplementaryPwm)
- Won't work: PA4(TIM3_CH2 needs remap=3), PB7(TIM1_CH2N needs remap=0/2), PC3(TIM1_CH4 needs remap=3)

### Firmware deps
- `ch32-hal` (git) with features: ch32x033f8p6, memory-x, embassy, rt
- `embassy-executor` with arch-spin executor (no time driver since TIM2 is used for PWM)
- Nightly Rust, target `riscv32imc-unknown-none-elf`

### GUI deps
- Tauri 2 with tray-icon, window-state plugins
- `serialport` for USB CDC
- `sysinfo` for CPU/memory/disk/network
- `nvml-wrapper` (Windows/Linux) or CoreFoundation (macOS) for GPU
- `reqwest` for Claude API usage fetching
- `nusb` for USB device enumeration (hotplug watch + firmware version reading)
