# pointify-firmware

CH32X033F8P6 firmware built on [ch32-hal](https://github.com/ch32-rs/ch32-hal) with [Embassy](https://embassy.dev/) async runtime.

USB CDC ACM is implemented with raw PAC registers (`pac::usb::Usbd`) since ch32-hal has no USBFS HAL driver for this chip.

## Features

- USB CDC ACM device (VID=0x0200, PID=0x02DB)
- 8-channel PWM output (TIM1/TIM2/TIM3)
- Auto-detect 3.3V/5V supply via ADC internal reference
- Voltage-aware PWM scaling (3V gauges on 5V supply are scaled down)

## Prerequisites

```bash
cargo install probe-rs-tools
```

## Build & Flash

```bash
cargo run --release
```

Build only:

```bash
cargo build --release
```
