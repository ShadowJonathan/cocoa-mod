use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub struct Choker {
    // Send buffer
    pub buf: VecDeque<(usize, Vec<u8>)>,

    // (MID, acked, retransmission counter, rtt, data)
    pub window: Vec<(usize, u8, Option<Duration>, Vec<u8>)>,

    pub rto: RTO,
    pub rto_start: Instant,
    pub rto_end: Instant,

    pub window_max: usize,
    pub window_state: WindowState,
}

impl Choker {
    pub fn new() -> Self {
        Self {
            buf: VecDeque::new(),
            window: Vec::new(),
            rto: RTO::new(),
            rto_start: Instant::now(),
            rto_end: Instant::now().checked_add(Duration::from_secs(2)).unwrap(),
            window_max: 1,
            window_state: WindowState::Halted,
        }
    }

    pub fn buf(&mut self) -> &mut VecDeque<(usize, Vec<u8>)> {
        &mut self.buf
    }

    pub fn set_ack(&mut self, mid: usize, rtt: Duration) {
        for (w_mid, _, d, _) in &mut self.window {
            if *w_mid == mid {
                *d = Some(rtt);
                break;
            }
        }
    }

    pub fn get_data<'a>(&'a self, mids: &'a [usize]) -> impl Iterator<Item = &[u8]> {
        self.window.iter().filter_map(|e| {
            if mids.contains(&e.0) {
                Some(e.3.as_slice())
            } else {
                None
            }
        })
    }

    // Ticks at rto_end, prunes window and calculates new rto,
    // retains unacked packets, fills window with new packets,
    // sets and returns new rto_end, and the MIDs of the packets to be (re)transmitted
    pub fn rto_tick(&mut self, now: Instant) -> (Instant, Vec<usize>) {
        let acked: usize = self.window[..self.relevant_window_len()].iter().filter(|e| e.2.is_some()).count();

        self.window.retain(|(_, transmissions, rtt, _)| {
            if let Some(rtt) = rtt {
                self.rto.calc(*transmissions, *rtt, acked);

                false
            } else {
                true
            }
        });

        let mut unacked: usize = 0;

        let len = self.relevant_window_len();
        for (_, t, _, _) in self.window[..len].iter_mut() {
            *t += 1;
            unacked += 1;
        }

        if unacked > 0 {
            self.window_state = WindowState::Halted;
            let packet_min = unacked / 2;

            if self.window_max > packet_min {
                self.window_max -= packet_min;
            } else {
                self.window_max = 1;
            }
        } else {
            if let WindowState::Rising { factor, conseq } = &mut self.window_state {
                if *factor == 0 {
                    // Start of the window accel
                    *factor += 1;
                } else {
                    *conseq += 1;
                }

                if *conseq == 3 {
                    *factor += 1;
                    *conseq = 0;
                }

                self.window_max += *factor;
            } else {
                self.window_state = WindowState::Rising {
                    factor: 0,
                    conseq: 0,
                }
            }
        }

        while self.window_max > self.window.len() {
            // fill the window with elements from the buffer
            if let Some((mid, data)) = self.buf.pop_back() {
                self.window.push((mid, 0, None, data))
            } else {
                break;
            }
        }

        self.rto_start = now;
        self.rto_end = self.rto_start + self.rto();

        let mids: Vec<_> = self.window[..self.relevant_window_len()]
            .iter().map(|e| e.0).collect();

        (self.rto_end, mids)
    }

    pub fn rto(&self) -> Duration {
        self.rto.rto
    }

    fn relevant_window_len(&self) -> usize {
        usize::min(self.window_max, self.window.len())
    }
}

const ALPHA: f64 = 0.125;
const BETA: f64 = 0.25;
const W_STRONG: f64 = 0.5;
const W_WEAK: f64 = 0.25;
pub struct RTO {
    pub rto: Duration,

    pub var_strong: f64,
    pub strong: f64,

    pub var_weak: f64,
    pub weak: f64,
}

impl RTO {
    pub fn new() -> Self {
        RTO {
            rto: Duration::from_secs(2),
            var_strong: 0.2,
            strong: 2.0,
            var_weak: 0.2,
            weak: 2.0,
        }
    }

    pub fn calc(&mut self, transmissions: u8, rtt: Duration, weighted_against: usize) {
        let mut secs = self.rto.as_secs_f64();
        let rtt = rtt.as_secs_f64();
        let weight = weighted_against as f64;

        if transmissions == 0 {
            // instant ACK

            // Update self.var_strong with a 4th of (self.strong - rtt)
            self.var_strong = bias(self.var_strong, BETA / weight, self.strong - rtt);

            // Bias self.strong with an 8th of rtt
            self.strong = bias(self.strong, ALPHA / weight, rtt);

            // Update secs with a bias of (self.strong + 4*self.var_strong)
            secs = bias(
                secs,
                W_STRONG / weight,
                self.strong + (4.0 * self.var_strong),
            );
        } else if transmissions <= 2 {
            // took 1 or 2 retransmissions

            // Update self.weak with a 4th of (self.weak - rtt)
            self.var_weak = bias(self.var_weak, BETA / weight, self.weak - rtt);

            self.weak = bias(self.weak, ALPHA / weight, rtt);

            secs = bias(secs, W_WEAK / weight, self.weak + self.var_weak);
        }

        self.rto = Duration::from_secs_f64(secs);
    }
}

// a gets `1 - weight` influence, b gets `weight` influence
pub fn bias(a: f64, weight: f64, b: f64) -> f64 {
    ((1.0 - weight) * a) + (b * weight)
}

pub enum WindowState {
    Rising { factor: usize, conseq: u8 },
    Halted,
}
