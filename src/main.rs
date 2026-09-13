#![no_main]
#![no_std]

use cortex_m::peripheral::NVIC;
use defmt_rtt as _;
use efm32xg_hal::{
    cmu::{Cmu, HfClockSource, LfClockSource},
    dma::Dma,
    gpio::{dynamic::DynamicPin, efemb::AsyncInputPin},
    pac::Interrupt,
    peripherals::Usart0,
    prelude::*,
    timer_le::efemb::Ticker,
    usart::spi::{BitOrder, Config, SpiParts},
};
use embassy_executor::Spawner;
use embassy_time::Timer;
use embedded_hal::spi::MODE_2;
use embedded_hal_async::digital::Wait;
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
    let clk = gpio.pc8.into_mode::<OutPp>();
    let tx = gpio.pc6.into_mode::<OutPp>();
    let rx = gpio.pc7.into_mode::<InFilt>();
    let spi_parts = SpiParts::new(p.Usart0, clk, tx, rx);
    let spi_config = Config::new(MODE_2, 0)
        .with_loopback(true)
        .with_bit_order(BitOrder::MsbFirst);
    let spi: Spi<'static, Usart0> = Spi::new(spi_parts, &spi_config, dma.ch1, dma.ch0);

    // Spawn tasks
    spawner.spawn(button_led_task(btn0, led0).expect("Could not spawn Task 0"));
    spawner.spawn(button_led_task(btn1, led1).expect("Could not spawn Task 1"));
    spawner.spawn(spi_demo_task(spi).expect("Could not spawn SPI task"));

    defmt::info!("EFM32XG Demo started!");
    defmt::info!("Press BTN0 (PF6) or BTN1 (PF7) to toggle LEDs");
    defmt::info!("SPI loopback test running...");
}

#[embassy_executor::task(pool_size = 2)]
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

#[embassy_executor::task]
async fn spi_demo_task(spi: Spi<'static, Usart0>) {
    // We need to make the SPI mutable for the transfer
    // But we can't make it mutable in the task signature because task args must be 'static
    // So we'll use a local mutable reference
    let mut spi = spi;

    // Test data for SPI loopback
    let test_data = [0x55, 0xAA, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06];
    let tx_buf = test_data;
    let mut rx_buf = [0u8; 8];

    loop {
        defmt::info!("Starting SPI loopback test...");

        // Transfer data in loopback mode (TX should be received back)
        match spi.transfer_async(&mut rx_buf, &tx_buf).await {
            Ok(_) => {
                // Check if we got back what we sent
                if rx_buf == test_data {
                    defmt::info!("SPI loopback test PASSED! Received: {:02X}", rx_buf);
                } else {
                    defmt::error!("SPI loopback test FAILED!");
                    defmt::error!("Sent:    {:02X}", tx_buf);
                    defmt::error!("Received: {:02X}", rx_buf);
                }
            }
            Err(e) => {
                defmt::error!("SPI transfer error: {:?}", e);
            }
        }

        // Wait before next test
        Timer::after_secs(1).await;
    }
}
