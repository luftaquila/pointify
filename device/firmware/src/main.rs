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

    // TIM3 remap=2: PC19
    let ch1_t3 = PwmPin::new_ch1::<2>(p.PC19);
    let mut pwm_tim3 = SimplePwm::new(
        p.TIM3, Some(ch1_t3), None, None, None,
        Hertz::khz(1), CountingMode::default(),
    );
    pwm_tim3.enable(Channel::Ch1);

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

    usb_cdc::usb_init();

    let max_duty_tim2 = pwm_tim2.get_max_duty();
    let max_duty_tim3 = pwm_tim3.get_max_duty();
    let max_duty_tim1 = pwm_tim1.get_max_duty();

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
                    0 => pwm_tim2.set_duty(Channel::Ch1, duty_u32(value, max_duty_tim2, scale_3v)),
                    1 => pwm_tim2.set_duty(Channel::Ch2, duty_u32(value, max_duty_tim2, scale_3v)),
                    2 => pwm_tim2.set_duty(Channel::Ch3, duty_u32(value, max_duty_tim2, scale_3v)),
                    3 => pwm_tim2.set_duty(Channel::Ch4, duty_u32(value, max_duty_tim2, scale_3v)),
                    4 => pwm_tim3.set_duty(Channel::Ch1, duty_u32(value, max_duty_tim3, scale_3v)),
                    8 => pwm_tim1.set_duty(Channel::Ch3, duty_u16(value, max_duty_tim1, scale_3v)),
                    9 => pwm_tim1.set_duty(Channel::Ch1, duty_u16(value, max_duty_tim1, scale_3v)),
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
