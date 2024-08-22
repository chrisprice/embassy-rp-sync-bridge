#![no_std]
#![no_main]

use cortex_m::delay::Delay;
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::{clocks::clk_sys_freq, multicore::Stack};
use embassy_rp_sync_bridge::State;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

static CORE1_STACK: StaticCell<Stack<4096>> = StaticCell::new();
static STATE: StaticCell<State<usize, usize, 1, 1>> = StaticCell::new();

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    info!("Hello, world!");
    let p = embassy_rp::init(Default::default());

    let core1_stack = CORE1_STACK.init(Stack::new());
    let state = STATE.init(State::new());

    let (tx, rx) = embassy_rp_sync_bridge::spawn(
        p.CORE1,
        core1_stack,
        state,
        move |bidi_channel, syst| loop {
            let mut delay = Delay::new(syst, clk_sys_freq());
            // loop until there's an item in the channel
            loop {
                match bidi_channel.receive() {
                    Ok(item) => {
                        info!("Received on core 1: {}", item);
                        delay.delay_ms(1000);
                        // loop until there's space in the channel
                        loop {
                            match bidi_channel.send(item) {
                                Ok(_) => break,
                                Err(_) => continue,
                            }
                        }
                    }
                    Err(_) => continue,
                }
            }
        },
    );

    loop {
        tx.send(42).await;
        let item = rx.receive().await;
        info!("Received on core 0: {}", item);
    }
}
