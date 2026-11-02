use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct SloSnapshot {
    pub name: String,
    pub target: f64,
    pub good: u64,
    pub bad: u64,
    pub sli: f64,
    pub burn_rate: f64,
}
#[derive(Clone, Debug)]
pub struct BurnAlert {
    pub alerting: bool,
    pub short_burn_rate: f64,
    pub long_burn_rate: f64,
}
#[derive(Clone, Debug)]
pub struct SloForecast {
    pub alerting: bool,
    pub projected_sli: f64,
}

pub struct SloTracker {
    name: String,
    target: f64,
    window: Duration,
    events: VecDeque<(Instant, bool)>,
}

impl SloTracker {
    pub fn new(name: impl Into<String>, target: f64, window: Duration) -> Self {
        Self {
            name: name.into(),
            target,
            window,
            events: VecDeque::new(),
        }
    }
    pub fn record(&mut self, good: bool) {
        self.events.push_back((Instant::now(), good));
        self.prune();
    }
    pub fn snapshot(&mut self) -> SloSnapshot {
        self.prune();
        let good = self.events.iter().filter(|(_, good)| *good).count() as u64;
        let bad = self.events.len() as u64 - good;
        let sli = if self.events.is_empty() {
            f64::NAN
        } else {
            good as f64 / self.events.len() as f64
        };
        let burn_rate = if self.events.is_empty() {
            0.0
        } else {
            (bad as f64 / self.events.len() as f64) / (1.0 - self.target)
        };
        SloSnapshot {
            name: self.name.clone(),
            target: self.target,
            good,
            bad,
            sli,
            burn_rate,
        }
    }
    pub fn forecast(&mut self, expected_events: u64, failure_fraction: f64) -> SloForecast {
        let current = self.snapshot();
        let projected_bad =
            current.bad as f64 + (expected_events as f64 * failure_fraction).round();
        let projected_sli =
            1.0 - projected_bad / (current.good + current.bad + expected_events) as f64;
        SloForecast {
            alerting: projected_sli < self.target,
            projected_sli,
        }
    }
    pub fn evaluate(
        short: &SloSnapshot,
        long: &SloSnapshot,
        short_threshold: f64,
        long_threshold: f64,
    ) -> BurnAlert {
        BurnAlert {
            alerting: short.burn_rate >= short_threshold && long.burn_rate >= long_threshold,
            short_burn_rate: short.burn_rate,
            long_burn_rate: long.burn_rate,
        }
    }
    fn prune(&mut self) {
        while self
            .events
            .front()
            .is_some_and(|(time, _)| time.elapsed() > self.window)
        {
            self.events.pop_front();
        }
    }
}
