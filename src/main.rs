#![no_main]
#![no_std]

use crate::display::ls013b7dh03::DisplayFrame;
use cortex_m::peripheral::NVIC;
use defmt::info;
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
use embassy_time::{Instant, Timer};
use embedded_graphics::{
    draw_target::DrawTarget,
    geometry::{Dimensions, Point},
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    pixelcolor::BinaryColor,
    primitives::{Primitive, PrimitiveStyle, PrimitiveStyleBuilder, StrokeAlignment, Triangle},
    text::{Alignment, Text},
    Drawable,
};
use embedded_hal::spi::MODE_0;
use embedded_hal_async::digital::Wait;
use panic_probe as _;

mod display;

const MAX_DISPLAY_FRAME_COUNT: usize = 2;
static TO_SPI: Channel<ThreadModeRawMutex, DisplayFrame, MAX_DISPLAY_FRAME_COUNT> = Channel::new();
static FROM_SPI: Channel<ThreadModeRawMutex, DisplayFrame, MAX_DISPLAY_FRAME_COUNT> =
    Channel::new();

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

    // ---- SPI Task ----
    spawner.spawn(
        spi_demo_task(
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

    defmt::info!("EFM32XG Demo started!");
    defmt::info!("Press BTN0 (PF6) or BTN1 (PF7) to toggle LEDs");

    let frames_in = FROM_SPI.receiver();
    let frames_out = TO_SPI.sender();

    let disp_frame = display::ls013b7dh03::take_display_frame();
    frames_out.send(disp_frame).await;

    let mut color = false;
    // Create styles used by the drawing operations.
    let thin_stroke = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let border_stroke = PrimitiveStyleBuilder::new()
        .stroke_color(BinaryColor::On)
        .stroke_width(3)
        .stroke_alignment(StrokeAlignment::Inside)
        .build();
    let character_style = MonoTextStyle::new(&FONT_6X10, BinaryColor::On);
    let yoffset = 10;

    let sec_duration: f32 = 1000.0;
    let mut prev_frame_start = Instant::now();
    loop {
        let mut frame = frames_in.receive().await;
        let frame_start = Instant::now();
        let frame_duration = (frame_start - prev_frame_start).as_millis() as f32;
        prev_frame_start = frame_start;

        let fps = if frame_duration.is_normal() {
            (((sec_duration / frame_duration) * 100.0) as u16) as f32 / 100.0
        } else {
            0.0
        };
        info!("{=f32} fps, {} ms", &fps, frame_duration);

        color = !color;
        frame.clear(BinaryColor::Off);

        // Draw a 3px wide outline around the display.
        frame
            .bounding_box()
            .into_styled(border_stroke)
            .draw(&mut frame);

        // Draw a triangle.
        Triangle::new(
            Point::new(16, 16 + yoffset),
            Point::new(16 + 16, 16 + yoffset),
            Point::new(16 + 8, yoffset),
        )
        .into_styled(thin_stroke)
        .draw(&mut frame);

        // Draw centered text.
        let text = "embedded-graphics";
        Text::with_alignment(
            text,
            frame.bounding_box().center() + Point::new(0, 15),
            character_style,
            Alignment::Center,
        )
        .draw(&mut frame);

        frames_out.send(frame).await;
    }
}

#[task(pool_size = 2)]
async fn button_led_task(btn_id: ButtonId, mut btn: AsyncInputPin, mut led: DynamicPin) {
    loop {
        // Wait for button press (active low)
        let _ = btn.wait_for_low().await;
        let _ = led.set_high();
        defmt::info!("{} pressed - LED ON", &btn_id);

        // Small delay to debounce
        Timer::after_millis(50).await;

        // Wait for button release
        let _ = btn.wait_for_high().await;
        let _ = led.set_low();
        defmt::info!("{} released - LED OFF", &btn_id);

        // Small delay to debounce
        Timer::after_millis(50).await;
    }
}

#[task]
async fn spi_demo_task(
    frames_in: Receiver<
        'static,
        ThreadModeRawMutex,
        DisplayFrame<'static>,
        MAX_DISPLAY_FRAME_COUNT,
    >,
    frames_out: Sender<'static, ThreadModeRawMutex, DisplayFrame<'static>, MAX_DISPLAY_FRAME_COUNT>,
    mut spi: Spi<'static, Usart0>,
    mut cs: Pin<'D', 14, OutPp>,
    _disp_com: Pin<'D', 13, OutPp>,
) {
    defmt::info!("Started SPI task...");

    loop {
        let frame = frames_in.receive().await;

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

        frames_out.send(frame).await;
    }
}
