#![no_main]
#![no_std]

mod display;

use crate::display::DisplayFrame;
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
use embassy_sync::{
    blocking_mutex::raw::ThreadModeRawMutex,
    channel::{Channel, Receiver, Sender},
};
use embassy_time::Timer;
use embedded_hal::spi::MODE_0;
use embedded_hal_async::digital::Wait;
use panic_probe as _;

const MAX_DISPLAY_BUFFER_COUNT: usize = 2;

type DisplayFrameCh = Channel<
    ThreadModeRawMutex,
    DisplayFrame<'static, { display::ls013b7dh03::BUF_SIZE }>,
    MAX_DISPLAY_BUFFER_COUNT,
>;
type DisplayFrameChReceiver = Receiver<
    'static,
    ThreadModeRawMutex,
    DisplayFrame<'static, { display::ls013b7dh03::BUF_SIZE }>,
    MAX_DISPLAY_BUFFER_COUNT,
>;
type DisplayFrameChSender = Sender<
    'static,
    ThreadModeRawMutex,
    DisplayFrame<'static, { display::ls013b7dh03::BUF_SIZE }>,
    MAX_DISPLAY_BUFFER_COUNT,
>;

static TO_SPI: DisplayFrameCh = Channel::new();
static FROM_SPI: DisplayFrameCh = Channel::new();

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ButtonId {
    Btn0,
    Btn1,
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = efm32xg_hal::init();

    // Initialize clocks
    let _clocks = Cmu::new(p.Cmu)
        .with_hf_clk(HfClockSource::HfXO(40_000_000), HfClockPrescaler::Div1)
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

    // Let this App take control of display (this is a `UG154: EFM32 Pearl Gecko Starter Kit` paticularity)
    let _ = gpio.pd15.into_mode::<OutPp>().set_high();

    // Add `DisplayFrame`s to the display task queue
    for f in display::ls013b7dh03::take_display_frames() {
        FROM_SPI.sender().send(f).await;
    }

    // ---- LED 0 and Button 0 Task ----
    spawner.spawn(
        button_led_task(
            ButtonId::Btn0,
            gpio.pf6
                .into_mode::<InFloat>()
                .into_async_input(gpio.exti4ctrl),
            gpio.pf4.into_mode::<OutPp>().into_dynamic_pin(),
        )
        .expect("Could not spawn Task 0"),
    );

    // ---- LED 1 and Button 1 Task ----
    spawner.spawn(
        button_led_task(
            ButtonId::Btn1,
            gpio.pf7
                .into_mode::<InFilt>()
                .into_async_input(gpio.exti5ctrl),
            gpio.pf5.into_mode::<OutPpAlt>().into_dynamic_pin(),
        )
        .expect("Could not spawn Task 1"),
    );

    // ---- LCD SPI Task ----
    spawner.spawn(
        lcd_task(
            TO_SPI.receiver(),
            FROM_SPI.sender(),
            Spi::new(
                SpiParts::new(
                    p.Usart0,
                    gpio.pc8.into_mode::<OutPp>(),
                    gpio.pc6.into_mode::<OutPp>(),
                    gpio.pc7.into_mode::<InFilt>(),
                ),
                &Config::new(MODE_0, 16).with_bit_order(BitOrder::MsbFirst),
                dma.ch1,
                dma.ch0,
            ),
            gpio.pd14.into_mode::<OutPp>(),
            gpio.pd13.into_mode::<OutPp>(),
        )
        .expect("Could not spawn SPI task"),
    );

    // ---- Display Task ----
    spawner.spawn(
        display_task(FROM_SPI.receiver(), TO_SPI.sender()).expect("Could not spawn Display task"),
    );
}

#[task(pool_size = 2)]
async fn button_led_task(btn_id: ButtonId, mut btn: AsyncInputPin, mut led: DynamicPin) {
    loop {
        // Wait for button press (active low)
        let _ = btn.wait_for_low().await;
        let _ = led.set_high();
        defmt::info!("{} pressed", &btn_id);

        // Small delay to debounce
        Timer::after_millis(50).await;

        // Wait for button release
        let _ = btn.wait_for_high().await;
        let _ = led.set_low();
        defmt::info!("{} released", &btn_id);

        // Small delay to debounce
        Timer::after_millis(50).await;
    }
}

#[task]
async fn lcd_task(
    frames_in: DisplayFrameChReceiver,
    frames_out: DisplayFrameChSender,
    spi: Spi<'static, Usart0>,
    cs: Pin<'D', 14, OutPp>,
    disp_com_inv: Pin<'D', 13, OutPp>,
) {
    defmt::info!("Started SPI task...");
    display::ls013b7dh03::lcd_task(frames_in, frames_out, spi, cs, disp_com_inv).await;
}

#[task]
async fn display_task(frames_in: DisplayFrameChReceiver, frames_out: DisplayFrameChSender) {
    defmt::info!("Started Display task...");

    display::display_task(frames_in, frames_out).await;
}
