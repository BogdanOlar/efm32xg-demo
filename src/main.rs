#![no_main]
#![no_std]

use cortex_m::peripheral::NVIC;
use defmt_rtt as _;
use efm32xg_hal::{
    cmu::{Cmu, HfClockSource, LfClockSource},
    dma::Dma,
    gpio::{dynamic::DynamicPin, efemb::AsyncInputPin, OutPp, Pin},
    pac::Interrupt,
    peripherals::Usart0,
    prelude::*,
    timer_le::efemb::Ticker,
    usart::spi::{BitOrder, Config, SpiParts},
};
use embassy_executor::{task, Spawner};
use embassy_time::Timer;
use embedded_hal::spi::MODE_0;
use embedded_hal_async::digital::Wait;
use ls013b7dh03::Ls013b7dh03;
use panic_probe as _;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = efm32xg_hal::init();

    // Initialize clocks
    let _clocks = Cmu::new(p.Cmu)
        .with_hf_clk(HfClockSource::HfRco, HfClockPrescaler::Div1)
        .with_lfa_clk(LfClockSource::LfRco)
        .freeze();

    // Initialize embassy time driver
    Ticker::init();

    let gpio = Gpio::new(p.Gpio);
    let dma = Dma::init(p.Ldma);

    // Enable NVIC for GPIO interrupts
    unsafe {
        NVIC::unmask(Interrupt::GPIO_EVEN);
        NVIC::unmask(Interrupt::GPIO_ODD);
    }

    // ---- LED 0 and Button 0 ----
    let led0 = gpio.pf4.into_mode::<OutPp>().into_dynamic_pin();
    let btn0 = gpio
        .pf6
        .into_mode::<InFloat>()
        .into_async_input(gpio.exti4ctrl);

    // ---- LED 1 and Button 1 ----
    let led1 = gpio.pf5.into_mode::<OutPpAlt>().into_dynamic_pin();
    let btn1 = gpio
        .pf7
        .into_mode::<InFilt>()
        .into_async_input(gpio.exti5ctrl);

    // ---- SPI Setup ----
    let spi: Spi<'static, Usart0> = Spi::new(
        SpiParts::new(
            p.Usart0,
            gpio.pc8.into_mode::<OutPp>(),
            gpio.pc6.into_mode::<OutPp>(),
            gpio.pc7.into_mode::<InFilt>(),
        ),
        &Config::new(MODE_0, 32).with_bit_order(BitOrder::MsbFirst),
        dma.ch1,
        dma.ch0,
    );
    let cs = gpio.pd14.into_mode::<OutPp>();
    let disp_com = gpio.pd13.into_mode::<OutPp>();
    // Let this App take control of display (this is a `UG154: EFM32 Pearl Gecko Starter Kit` paticularity)
    let _ = gpio.pd15.into_mode::<OutPp>().set_high();

    // Spawn tasks
    spawner.spawn(button_led_task(btn0, led0).expect("Could not spawn Task 0"));
    spawner.spawn(button_led_task(btn1, led1).expect("Could not spawn Task 1"));
    spawner.spawn(spi_demo_task(spi, cs, disp_com).expect("Could not spawn SPI task"));

    defmt::info!("EFM32XG Demo started!");
    defmt::info!("Press BTN0 (PF6) or BTN1 (PF7) to toggle LEDs");
    defmt::info!("SPI loopback test running...");
}

#[task(pool_size = 2)]
async fn button_led_task(mut btn: AsyncInputPin, mut led: DynamicPin) {
    loop {
        // Wait for button press (active low)
        let _ = btn.wait_for_low().await;
        let _ = led.set_high();
        defmt::info!("Button pressed - LED ON");

        // Small delay to debounce
        Timer::after_millis(50).await;

        // Wait for button release
        let _ = btn.wait_for_high().await;
        let _ = led.set_low();
        defmt::info!("Button released - LED OFF");

        // Small delay to debounce
        Timer::after_millis(50).await;
    }
}

#[task]
async fn spi_demo_task(
    spi: Spi<'static, Usart0>,
    cs: Pin<'D', 14, OutPp>,
    disp_com: Pin<'D', 13, OutPp>,
) {
    // Test data for SPI loopback
    let mut buffer = [0u8; ls013b7dh03::BUF_SIZE];
    let mut disp = Ls013b7dh03::new(spi, cs, disp_com, &mut buffer);

    defmt::info!("Starting SPI loopback test...");

    loop {
        for y in 0..ls013b7dh03::HEIGHT as u8 {
            for x in 0..ls013b7dh03::WIDTH as u8 {
                let write_ret = disp.write(x, y, true);
                assert!(write_ret.is_ok());
            }
        }

        // Update the display
        disp.flush();

        // Wait before next test
        Timer::after_secs(1).await;

        for y in 0..ls013b7dh03::HEIGHT as u8 {
            for x in 0..ls013b7dh03::WIDTH as u8 {
                let write_ret = disp.write(x, y, false);

                assert!(write_ret.is_ok());
            }
        }

        // Update the display
        disp.flush();

        // Wait before next test
        Timer::after_secs(1).await;
    }
}
