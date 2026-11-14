#![no_main]

use libfuzzer_sys::fuzz_target;

// The normal integration test calls the same runner; a crashing input is
// replayable with cargo fuzz run peer_network <artifact>.
#[path = "../../tests/support/peer_sim.rs"]
mod peer_sim;

fuzz_target!(|data: &[u8]| {
    peer_sim::run_fault_case(data);
});
