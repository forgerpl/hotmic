use crossbeam_channel;
use mio::{Poll, Events, Ready, Token, PollOpt};
use channel;
use configuration::Configuration;
use control::{ControlMessage, Controller};
use sink::Sink;
use data::{Facet, Sample, Counter, Gauge, Histogram, Snapshot, Percentile, default_percentiles};
use std::hash::Hash;
use std::fmt::Display;
use std::time::{Instant, Duration};
use std::collections::HashSet;

const DATA: Token = Token(5);
const CONTROL: Token = Token(15);

/// Metrics receiver which aggregates and processes samples.
pub struct Receiver<T> {
    conf: Configuration<T>,

    // Sample aggregation machinery.
    poll: Poll,
    buffer_pool_tx: crossbeam_channel::Sender<Vec<Sample<T>>>,
    buffer_pool_rx: crossbeam_channel::Receiver<Vec<Sample<T>>>,
    data_tx: channel::Sender<Vec<Sample<T>>>,
    data_rx: channel::Receiver<Vec<Sample<T>>>,
    control_tx: channel::Sender<ControlMessage<T>>,
    control_rx: channel::Receiver<ControlMessage<T>>,
    facets: HashSet<Facet<T>>,

    // Metric machinery.
    counter: Counter<T>,
    gauge: Gauge<T>,
    histogram: Histogram<T>,
    percentiles: Vec<Percentile>,
    last_upkeep: Instant,
}

impl<T: Send + Eq + Hash + Display + Clone> Receiver<T> {
    pub(crate) fn from_config(conf: Configuration<T>) -> Receiver<T> {
        // Create our data, control, and buffer channels.
        let (data_tx, data_rx) = channel::channel::<Vec<Sample<T>>>(conf.capacity);
        let (control_tx, control_rx) = channel::channel::<ControlMessage<T>>(conf.capacity);
        let (buffer_pool_tx, buffer_pool_rx) = crossbeam_channel::bounded::<Vec<Sample<T>>>(conf.capacity);

        // Pre-allocate our sample batch buffers and put them into the buffer channel.
        for _ in 0..conf.capacity {
            let _ = buffer_pool_tx.send(Vec::with_capacity(conf.batch_size));
        }

        // Configure our poller.
        let poll = Poll::new().unwrap();
        poll.register(&data_rx, DATA, Ready::readable(), PollOpt::level()).unwrap();
        poll.register(&control_rx, CONTROL, Ready::readable(), PollOpt::level()).unwrap();

        Receiver {
            conf: conf,
            poll: poll,
            buffer_pool_rx: buffer_pool_rx,
            buffer_pool_tx: buffer_pool_tx,
            data_tx: data_tx,
            data_rx: data_rx,
            control_tx: control_tx,
            control_rx: control_rx,
            facets: HashSet::new(),
            counter: Counter::new(),
            gauge: Gauge::new(),
            histogram: Histogram::new(Duration::from_secs(10), Duration::from_secs(1)),
            percentiles: default_percentiles(),
            last_upkeep: Instant::now(),
        }
    }

    /// Gets a builder to configure a `Receiver` instance with.
    pub fn builder() -> Configuration<T> {
        Configuration::default()
    }

    /// Creates a `Sink` bound to this receiver.
    pub fn get_sink(&self) -> Sink<T> {
        Sink::new(
            self.buffer_pool_rx.clone(),
            self.data_tx.clone(),
            self.control_tx.clone(),
            self.conf.batch_size,
        )
    }

    /// Creates a `Controller` bound to this receiver.
    pub fn get_controller(&self) -> Controller<T> {
        Controller::new(self.control_tx.clone())
    }

    /// Run the receiver for a single turn.
    ///
    /// A single turn involves performing upkeep (adjusting histograms to make sure their windowing
    /// is correct) and doing a single poll to see if any new data or control messages are
    /// available.
    ///
    /// By default, the poll delay, or how long the call to `poll` will wait before timing out, is
    /// set at 100ms.  This is important as we want to ensure the `poll` eventually returns during
    /// periods of inactivity so it can be be recalled (when running via `run`) and perform
    /// continued upkeep.
    pub fn turn(&mut self) {
        // Run upkeep before doing anything else.
        let now = Instant::now();
        if now >= self.last_upkeep + Duration::from_millis(250) {
            self.histogram.upkeep(now);
        }

        let mut events = Events::with_capacity(1024);
        self.poll.poll(&mut events, self.conf.poll_delay).unwrap();
        for event in events.iter() {
            let token = event.token();
            if token == DATA {
                if let Ok(mut results) = self.data_rx.recv() {
                    for result in &results {
                        self.counter.update(result);
                        self.gauge.update(result);
                        self.histogram.update(result);
                    }
                    results.clear();
                    let _ = self.buffer_pool_tx.send(results);
                }
            } else if token == CONTROL {
                if let Ok(msg) = self.control_rx.recv() {
                    match msg {
                        ControlMessage::AddFacet(facet) => self.add_facet(facet),
                        ControlMessage::RemoveFacet(facet) => self.remove_facet(facet),
                        ControlMessage::Snapshot(tx) => {
                            let mut snapshot = Snapshot::new();
                            for facet in &self.facets {
								match *facet {
									Facet::Count(ref key) => {
										snapshot.set_count(
											key.clone(),
											self.counter.value(key.clone())
										);
									},
                                    Facet::Gauge(ref key) => {
                                        snapshot.set_value(
                                            key.clone(),
                                            self.gauge.value(key.clone())
                                        );
                                    },
                                    Facet::TimingPercentile(ref key) => {
                                        match self.histogram.snapshot(key.clone()) {
                                            Some(hs) => {
                                                snapshot.set_timing_percentiles(key.clone(), hs, &self.percentiles)
                                            },
                                            None => {},
                                        }
                                    },
                                    Facet::ValuePercentile(ref key) => {
                                        match self.histogram.snapshot(key.clone()) {
                                            Some(hs) => {
                                                snapshot.set_value_percentiles(key.clone(), hs, &self.percentiles)
                                            },
                                            None => {},
                                        }
                                    },
								}
							}
                            let _ = tx.send(snapshot);
                        },
                    }
                }
            }
        }
    }

    /// Runs the receiver endlessly.
    pub fn run(&mut self) {
        loop {
            self.turn();
        }
    }

    /// Registers a facet with the receiver.
    pub fn add_facet(&mut self, facet: Facet<T>) {
        match facet.clone() {
            Facet::Count(t) => self.counter.register(t),
            Facet::Gauge(t) => self.gauge.register(t),
            Facet::TimingPercentile(t) => self.histogram.register(t),
            Facet::ValuePercentile(t) => self.histogram.register(t),
        }

        self.facets.insert(facet);
    }

    /// Deregisters a facet from the receiver.
    pub fn remove_facet(&mut self, facet: Facet<T>) {
        match facet.clone() {
            Facet::Count(t) => self.counter.deregister(t),
            Facet::Gauge(t) => self.gauge.deregister(t),
            Facet::TimingPercentile(t) => self.histogram.deregister(t),
            Facet::ValuePercentile(t) => self.histogram.deregister(t),
        }

        self.facets.remove(&facet);
    }
}
