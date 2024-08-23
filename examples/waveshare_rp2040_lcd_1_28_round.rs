#![no_std]
#![no_main]

use core::cell::RefCell;

use defmt::*;
use eg_seven_segment::{Digit, SevenSegmentStyleBuilder};
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embassy_executor::Spawner;
use embassy_rp::{
    clocks,
    gpio::{Level, Output},
    multicore::Stack,
    spi::{Blocking, Spi},
};
use embassy_rp_sync_bridge::State;
use embassy_sync::blocking_mutex::{raw::NoopRawMutex, Mutex};
use embassy_time::{Duration, Ticker};
use embedded_hal::delay::DelayNs;
use gc9a01::{prelude::DisplayResolution240x240, Gc9a01, SPIDisplayInterface};
use static_cell::{ConstStaticCell, StaticCell};
use {defmt_rtt as _, panic_probe as _};

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyleBuilder, Rectangle, StrokeAlignment},
};

use gc9a01::prelude::*;

static CORE1_STACK: ConstStaticCell<Stack<196608>> = ConstStaticCell::new(Stack::new());
static STATE: StaticCell<State<u32, (), 1, 1>> = StaticCell::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("Hello, world!");
    let p = embassy_rp::init(Default::default());

    let spi = p.SPI1;
    let clk_pin = p.PIN_10;
    let mosi_pin = p.PIN_11;
    let cs_output = Output::new(p.PIN_9, Level::Low);
    let dc_output = Output::new(p.PIN_8, Level::Low);
    let mut reset_output = Output::new(p.PIN_12, Level::High);
    let mut backlight_output = Output::new(p.PIN_25, Level::Low);

    let core1_stack = CORE1_STACK.take();
    let state = STATE.init(State::new());

    let (tx, _) = embassy_rp_sync_bridge::spawn(
        p.CORE1,
        core1_stack,
        state,
        move |bidi_channel, mut delay| {
            let mut display_config = embassy_rp::spi::Config::default();
            display_config.frequency = clocks::clk_peri_freq() / 2;

            let spi: Spi<'_, _, Blocking> =
                Spi::new_blocking_txonly(spi, clk_pin, mosi_pin, display_config.clone());

            // NoopRawMutex can be used as the bus isn't shared between tasks/cores.
            // N.B. This is an blocking_mutex.
            let spi_bus: Mutex<NoopRawMutex, _> = Mutex::new(RefCell::new(spi));
            let spi_device = SpiDeviceWithConfig::new(&spi_bus, cs_output, display_config);

            let interface = SPIDisplayInterface::new(spi_device, dc_output);

            let mut display = Gc9a01::new(
                interface,
                DisplayResolution240x240,
                DisplayRotation::Rotate0,
            )
            .into_buffered_graphics();

            // Reset the display, clear it and then turn on the backlight.
            // This avoids showing whatever was in the RAM before the reset.
            display.reset(&mut reset_output, &mut delay).unwrap();
            display.init(&mut delay).unwrap();
            display.clear();
            display.flush().unwrap();
            delay.delay_ms(20);
            backlight_output.set_high();

            let screen_width = display.size().width;
            let screen_height = display.size().height;
            let digit_width = 48;
            let digit_height = digit_width * 2;
            let digit_spacing = 10;
            let segment_width = 10;

            let style = SevenSegmentStyleBuilder::new()
                .digit_size((digit_width, digit_height).into())
                .digit_spacing(digit_spacing)
                .segment_width(segment_width)
                .segment_color(Rgb565::CSS_DARK_GRAY)
                .build();

            loop {
                let Ok(value) = bidi_channel.receive() else {
                    continue;
                };

                // Clear only the area where the digits will be drawn
                Rectangle::with_center(
                    (screen_width as i32 / 2, screen_height as i32 / 2).into(),
                    (4 * digit_width + 3 * digit_spacing, digit_height).into(),
                )
                .into_styled(
                    PrimitiveStyleBuilder::new()
                        .fill_color(Rgb565::BLACK)
                        // HACK: work around the off by 1 error
                        .stroke_color(Rgb565::BLACK)
                        .stroke_width(1)
                        .stroke_alignment(StrokeAlignment::Outside)
                        .build(),
                )
                .draw(&mut display)
                .unwrap();

                let mut next: Point = (
                    (screen_width - 4 * digit_width - 3 * digit_spacing) as i32 / 2,
                    (screen_height - digit_height) as i32 / 2,
                )
                    .into();
                for i in 0..4 {
                    next = Digit::new(
                        char::from_digit(value / 10_u32.pow(3 - i) % 10, 10)
                            .unwrap()
                            .try_into()
                            .unwrap(),
                        next,
                    )
                    .into_styled(style)
                    .draw(&mut display)
                    .unwrap();
                }
                display.flush().unwrap();
            }
        },
    );

    let mut ticker = Ticker::every(Duration::from_hz(10));
    let mut counter = 0;
    loop {
        tx.send(counter).await;
        counter += 1;
        ticker.next().await;
    }
}
