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
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use embassy_time::Timer;
use embedded_hal::spi::MODE_0;
use embedded_hal_async::digital::Wait;
use panic_probe as _;

use crate::display::ls013b7dh03::DisplayFrame;

mod display;

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

    let disp_frame = display::ls013b7dh03::take_display_frame();

    // Spawn tasks
    spawner.spawn(button_led_task(btn0, led0).expect("Could not spawn Task 0"));
    spawner.spawn(button_led_task(btn1, led1).expect("Could not spawn Task 1"));
    spawner.spawn(spi_demo_task(disp_frame, spi, cs, disp_com).expect("Could not spawn SPI task"));

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
    mut frame: display::ls013b7dh03::DisplayFrame<'static>,
    mut spi: Spi<'static, Usart0>,
    mut cs: Pin<'D', 14, OutPp>,
    _disp_com: Pin<'D', 13, OutPp>,
) {
    defmt::info!("Starting SPI loopback test...");

    // let mut buffer: [u8; display::Ls013b7dh03::BUF_SIZE] = [0; _];
    // let mut frame = DisplayFrame::new(&mut buffer);
    // let mut frame = display::Ls013b7dh03::take_display_frame();

    let mut color = false;

    loop {
        color = !color;

        for y in 0..frame.height() as u8 {
            for x in 0..frame.width() as u8 {
                frame.write(x, y, color);
            }
        }

        // Update the display
        {
            // Assert CS
            let _ = cs.set_high();

            // Write update command
            let spi_ret = spi
                .transfer_async(&mut [], &[display::ls013b7dh03::LcdMode::Update as u8])
                .await;
            assert!(spi_ret.is_ok());

            // Write buffer
            let spi_ret = spi.transfer_async(&mut [], frame.buffer()).await;
            assert!(spi_ret.is_ok());

            // Write filler byte
            let spi_ret = spi
                .transfer_async(&mut [], &[display::ls013b7dh03::FILLER_BYTE])
                .await;
            assert!(spi_ret.is_ok());

            // Deassert CS
            let _ = cs.set_low();
        }

        // Wait before next test
        Timer::after_secs(1).await;
    }
}
