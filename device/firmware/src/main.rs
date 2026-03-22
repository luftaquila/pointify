#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

mod usb_cdc;

use ch32_hal as hal;
use embassy_executor::Spawner;
use hal::adc::{Adc, SampleTime, VrefInt};
use hal::time::Hertz;
use hal::timer::complementary_pwm::{ComplementaryPwm, ComplementaryPwmPin};
use hal::timer::low_level::CountingMode;
use hal::timer::simple_pwm::{PwmPin, SimplePwm};
use hal::timer::Channel;

#[embassy_executor::main(entry = "qingke_rt::entry")]
async fn main(_spawner: Spawner) -> ! {
    let mut config = hal::Config::default();
    config.rcc = hal::rcc::Config::SYSCLK_FREQ_48MHZ_HSI;
    let p = hal::init(config);

    // ADC
    let mut adc = Adc::new(p.ADC1, Default::default());
    let mut vrefint = VrefInt;
    let vref_raw = adc.convert(&mut vrefint, SampleTime::CYCLES11) as u32;
    let is_5v_supply = vref_raw < 1200;

    // TIM2 remap=0: PA0, PA1, PA2, PA3
    let ch1 = PwmPin::new_ch1::<0>(p.PA0);
    let ch2 = PwmPin::new_ch2::<0>(p.PA1);
    let ch3 = PwmPin::new_ch3::<0>(p.PA2);
    let ch4 = PwmPin::new_ch4::<0>(p.PA3);
    let mut pwm_tim2 = SimplePwm::new(
        p.TIM2, Some(ch1), Some(ch2), Some(ch3), Some(ch4),
        Hertz::khz(1), CountingMode::default(),
    );
    pwm_tim2.enable(Channel::Ch1);
    pwm_tim2.enable(Channel::Ch2);
    pwm_tim2.enable(Channel::Ch3);
    pwm_tim2.enable(Channel::Ch4);

    // Release PC18/DIO and PC19/DCK debug pins for GPIO/timer use
    // AFIO_PCFR1 bits[26:24] = sw_cfg = 0b100 (disable debug, release both pins)
    unsafe {
        let afio_pcfr1 = 0x4001_0004 as *mut u32;
        let val = afio_pcfr1.read_volatile();
        afio_pcfr1.write_volatile((val & !(0x07 << 24)) | (0x04 << 24));
    }

    // TIM3 remap=2: PC19(CH1), PC18(CH2)
    let ch1_t3 = PwmPin::new_ch1::<2>(p.PC19);
    let ch2_t3 = PwmPin::new_ch2::<2>(p.PC18);
    let mut pwm_tim3 = SimplePwm::new(
        p.TIM3, Some(ch1_t3), Some(ch2_t3), None, None,
        Hertz::khz(1), CountingMode::default(),
    );
    pwm_tim3.enable(Channel::Ch1);
    pwm_tim3.enable(Channel::Ch2);

    // TIM1 remap=1: PA7(CH1N), PB1(CH3N)
    let ch1n = ComplementaryPwmPin::new_ch1::<1>(p.PA7);
    let ch3n = ComplementaryPwmPin::new_ch3::<1>(p.PB1);
    let mut pwm_tim1 = ComplementaryPwm::new(
        p.TIM1, None, Some(ch1n), None, None, None, Some(ch3n), None,
        Hertz::khz(1), CountingMode::default(),
    );
    pwm_tim1.set_dead_time(0);
    pwm_tim1.enable(Channel::Ch1);
    pwm_tim1.enable(Channel::Ch3);

    // HAL ComplementaryPwm doesn't work properly with N-only channels.
    // Configure TIM1 entirely via raw registers.
    const TIM1: usize = 0x4001_2C00;
    unsafe {
        // CR1: CEN (bit 0) = 1 — start counter
        let cr1 = (TIM1 + 0x00) as *mut u32;
        let v = cr1.read_volatile();
        cr1.write_volatile(v | 1);
        // CCMR1: OC1M = PWM mode 2 (0b111), OC1PE = 1
        let ccmr1 = (TIM1 + 0x18) as *mut u32;
        let v = ccmr1.read_volatile();
        ccmr1.write_volatile((v & !(0x7 << 4)) | (0x7 << 4) | (1 << 3));
        // CCMR2: OC3M = PWM mode 2 (0b111), OC3PE = 1
        let ccmr2 = (TIM1 + 0x1C) as *mut u32;
        let v = ccmr2.read_volatile();
        ccmr2.write_volatile((v & !(0x7 << 4)) | (0x7 << 4) | (1 << 3));
        // CCER: CC1NE (bit 2) = 1, CC3NE (bit 10) = 1
        let ccer = (TIM1 + 0x20) as *mut u32;
        let v = ccer.read_volatile();
        ccer.write_volatile(v | (1 << 2) | (1 << 10));
        // BDTR: MOE (bit 15) = 1
        let bdtr = (TIM1 + 0x44) as *mut u32;
        let v = bdtr.read_volatile();
        bdtr.write_volatile(v | (1 << 15));
    }

    usb_cdc::usb_init();

    let max_duty_tim2 = pwm_tim2.get_max_duty();
    let max_duty_tim3 = pwm_tim3.get_max_duty();
    let max_duty_tim1 = unsafe { ((TIM1 + 0x2C) as *const u32).read_volatile() as u16 }; // ARR

    let mut rx_buf = [0u8; 64];
    loop {
        if let Some(len) = usb_cdc::usb_poll(&mut rx_buf) {
            for chunk in rx_buf[..len].chunks_exact(2) {
                let raw = u16::from_be_bytes([chunk[0], chunk[1]]);
                let is_5v_gauge = (raw & 0x8000) != 0;
                let index = ((raw >> 10) & 0x1F) as u8;
                let value = raw & 0x3FF;
                let scale_3v = is_5v_supply && !is_5v_gauge;

                match index {
                    0 => unsafe { ((TIM1 + 0x34) as *mut u32).write_volatile(duty_u16(value, max_duty_tim1, scale_3v) as u32) }, // CCR1 (PA7)
                    1 => unsafe { ((TIM1 + 0x3C) as *mut u32).write_volatile(duty_u16(value, max_duty_tim1, scale_3v) as u32) }, // CCR3 (PB1)
                    2 => pwm_tim3.set_duty(Channel::Ch1, duty_u32(value, max_duty_tim3, scale_3v)),
                    7 => pwm_tim3.set_duty(Channel::Ch2, duty_u32(value, max_duty_tim3, scale_3v)),
                    3 => pwm_tim2.set_duty(Channel::Ch4, duty_u32(value, max_duty_tim2, scale_3v)),
                    4 => pwm_tim2.set_duty(Channel::Ch3, duty_u32(value, max_duty_tim2, scale_3v)),
                    5 => pwm_tim2.set_duty(Channel::Ch2, duty_u32(value, max_duty_tim2, scale_3v)),
                    6 => pwm_tim2.set_duty(Channel::Ch1, duty_u32(value, max_duty_tim2, scale_3v)),
                    31 if value == 0x3FF => {
                        // Zero all PWMs as visual confirmation before bootloader
                        pwm_tim2.set_duty(Channel::Ch1, 0);
                        pwm_tim2.set_duty(Channel::Ch2, 0);
                        pwm_tim2.set_duty(Channel::Ch3, 0);
                        pwm_tim2.set_duty(Channel::Ch4, 0);
                        pwm_tim3.set_duty(Channel::Ch1, 0);
                        pwm_tim3.set_duty(Channel::Ch2, 0);
                        unsafe { ((TIM1 + 0x34) as *mut u32).write_volatile(0) }; // CCR1
                        unsafe { ((TIM1 + 0x3C) as *mut u32).write_volatile(0) }; // CCR3
                        usb_cdc::enter_bootloader();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn duty_u32(value: u16, max_duty: u32, scale_3v: bool) -> u32 {
    if scale_3v {
        (value as u32 * max_duty * 3) / (1023 * 5)
    } else {
        (value as u32 * max_duty) / 1023
    }
}

fn duty_u16(value: u16, max_duty: u16, scale_3v: bool) -> u16 {
    if scale_3v {
        ((value as u32 * max_duty as u32 * 3) / (1023 * 5)) as u16
    } else {
        ((value as u32 * max_duty as u32) / 1023) as u16
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
