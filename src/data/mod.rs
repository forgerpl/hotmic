use std::marker::PhantomData;
use std::time::Instant;
use fnv::FnvHashMap;
use std::hash::Hash;
use std::fmt::Display;
use hdrhistogram::Histogram as HdrHistogram;

pub mod counter;
pub mod gauge;
pub mod histogram;

pub(crate) use self::counter::Counter;
pub(crate) use self::gauge::Gauge;
pub(crate) use self::histogram::Histogram;

/// Type of computation against aggregated/processed samples.
///
/// Facets are, in essence, views over given metrics: a count tallys up all the counts for a given
/// metric to keep a single, globally-consistent value for all the matching samples seen, etc.
///
/// More simply, facets are essentially the same thing as a metric type: counters, gauges,
/// histograms, etc.  We treat them a little different because callers are never directly saying
/// that they want to change the value of a counter, or histogram, they're saying that for a given
/// metric type, they care about certain facets.
#[derive(PartialEq, Eq, Hash, Clone, Debug)]
pub enum Facet<T> {
    /// A count.
    ///
    /// This could be the number of timing samples seen for a given metric,
    /// or the total count if modified via count samples.
    Count(T),

    /// A gauge.
    ///
    /// Gauges are singluar values, and operate in last-write-wins mode.
    Gauge(T),

    /// Timing-specific percentiles.
    ///
    /// The histograms that back percentiles are currently hard-coded to track a windowed view of
    /// 60 seconds, with a 1 second interval.  That is, they only store the last 60 seconds worth
    /// of data that they've been given.
    TimingPercentile(T),

    /// Value-specific percentiles.
    ///
    /// The histograms that back percentiles are currently hard-coded to track a windowed view of
    /// 60 seconds, with a 1 second interval.  That is, they only store the last 60 seconds worth
    /// of data that they've been given.
    ValuePercentile(T),
}

/// A measurement.
///
/// Samples are the decoupled way of submitting data into the sink.  Likewise with facets, metric
/// types/measurements are slightly decoupled, into facets and samples, so that we can allow
/// slightly more complicated and unique combinations of facets.
///
/// If you wanted to track, all time, the count of a given metric, but also the distribution of the
/// metric, you would need both a counter and histogram, as histograms are windowed.  Instead of
/// creating two metric names, and having two separate calls, you can simply register both the
/// count and timing percentile facets, and make one call, and both things are tracked for you.
///
/// There are multiple sample types to support the different types of measurements, which each have
/// their own specific data they must carry.
#[derive(Debug)]
pub enum Sample<T>
{
    /// A timed sample.
    ///
    /// Includes the start and end times, as well as a count field.
    ///
    /// The count field can represent amounts integral to the event,
    /// such as the number of bytes processed in the given time delta.
    Timing(T, Instant, Instant, u64),

    /// A counter delta.
    ///
    /// The value is added directly to the existing counter, and so
    /// negative deltas will decrease the counter, and positive deltas
    /// will increase the counter.
    Count(T, i64),

    /// A single value, also known as a gauge.
    ///
    /// Values operate in last-write-wins mode.
    ///
    /// Values themselves cannot be incremented or decremented, so you
    /// must hold them externally before sending them.
    Value(T, u64),
}

/// A labeled percentile.
///
/// This represents a floating-point value from 0 to 100, with a string label to be used for
/// displaying the given percentile.
#[derive(Clone)]
pub struct Percentile(pub String, pub f64);

/// A default set of percentiles that should support most use cases.
///
/// Contains min (or 0.0), p50 (50.0), p90 (090.0), p99 (99.0), p999 (99.9) and max (100.0).
pub fn default_percentiles() -> Vec<Percentile> {
    let mut p = Vec::new();
    p.push(Percentile("min".to_owned(), 0.0));
    p.push(Percentile("p50".to_owned(), 50.0));
    p.push(Percentile("p90".to_owned(), 90.0));
    p.push(Percentile("p99".to_owned(), 99.0));
    p.push(Percentile("p999".to_owned(), 99.9));
    p.push(Percentile("max".to_owned(), 100.0));
    p
}

/// A point-in-time view of metric data.
pub struct Snapshot<T> {
    marker: PhantomData<T>,
    pub signed_data: FnvHashMap<String, i64>,
    pub unsigned_data: FnvHashMap<String, u64>,
}

impl<T: Send + Eq + Hash + Send + Display + Clone> Snapshot<T> {
    /// Creates an empty `Snapshot`.
    pub fn new() -> Snapshot<T> {
        Snapshot {
            marker: PhantomData,
            signed_data: FnvHashMap::default(),
            unsigned_data: FnvHashMap::default(),
        }
    }

    /// Stores a counter value for the given metric key.
    pub fn set_count(&mut self, key: T, value: i64) {
        let fkey = format!("{}_count", key);
        self.signed_data.insert(fkey, value);
    }

    /// Stores a gauge value for the given metric key.
    pub fn set_value(&mut self, key: T, value: u64) {
        let fkey = format!("{}_value", key);
        self.unsigned_data.insert(fkey, value);
    }

    /// Sets timing percentiles for the given metric key.
    ///
    /// From the given `HdrHistogram`, all the specific `percentiles` will be extracted and stored.
    pub fn set_timing_percentiles(&mut self, key: T, h: HdrHistogram<u64>, percentiles: &[Percentile]) {
        for percentile in percentiles {
            let fkey = format!("{}_ns_{}", key, percentile.0);
            let value = h.value_at_percentile(percentile.1);
            self.unsigned_data.insert(fkey, value);
        }
    }

    /// Sets value percentiles for the given metric key.
    ///
    /// From the given `HdrHistogram`, all the specific `percentiles` will be extracted and stored.
    pub fn set_value_percentiles(&mut self, key: T, h: HdrHistogram<u64>, percentiles: &[Percentile]) {
        for percentile in percentiles {
            let fkey = format!("{}_value_{}", key, percentile.0);
            let value = h.value_at_percentile(percentile.1);
            self.unsigned_data.insert(fkey, value);
        }
    }

    /// Gets the counter value for the given metric key.
    ///
    /// Returns `None` if the metric key has no counter value in this snapshot.
    pub fn count(&self, key: &T) -> Option<&i64> {
        let fkey = format!("{}_count", key);
        self.signed_data.get(&fkey)
    }

    /// Gets the gauge value for the given metric key.
    ///
    /// Returns `None` if the metric key has no gauge value in this snapshot.
    pub fn value(&self, key: &T) -> Option<&u64> {
        let fkey = format!("{}_value", key);
        self.unsigned_data.get(&fkey)
    }

    /// Gets the given timing percentile for given metric key.
    ///
    /// Returns `None` if the metric key has no value at the given percentile in this snapshot.
    pub fn timing_percentile(&self, key: &T, percentile: Percentile) -> Option<&u64> {
        let fkey = format!("{}_ns_{}", key, percentile.0);
        self.unsigned_data.get(&fkey)
    }

    /// Gets the given value percentile for the given metric key.
    ///
    /// Returns `None` if the metric key has no value at the given percentile in this snapshot.
    pub fn value_percentile(&self, key: &T, percentile: Percentile) -> Option<&u64> {
        let fkey = format!("{}_value_{}", key, percentile.0);
        self.unsigned_data.get(&fkey)
    }
}

#[cfg(test)]
mod tests {
    use super::{Snapshot, Percentile};
    use hdrhistogram::Histogram;

    #[test]
    fn test_snapshot_simple_set_and_get() {
        let key = "ok".to_owned();
        let mut snapshot = Snapshot::new();
        snapshot.set_count(key.clone(), 1);
        snapshot.set_value(key.clone(), 42);

        assert_eq!(snapshot.count(&key).unwrap(), &1);
        assert_eq!(snapshot.value(&key).unwrap(), &42);
    }

    #[test]
    fn test_snapshot_percentiles() {
        let mut snapshot = Snapshot::new();

        {
            let mut h1 = Histogram::<u64>::new_with_bounds(1, u64::max_value(), 3).unwrap();
            h1.saturating_record(500_000);
            h1.saturating_record(750_000);
            h1.saturating_record(1_000_000);
            h1.saturating_record(1_250_000);

            let tkey = "ok".to_owned();
            let mut tpercentiles = Vec::new();
            tpercentiles.push(Percentile("min".to_owned(), 0.0));
            tpercentiles.push(Percentile("p50".to_owned(), 50.0));
            tpercentiles.push(Percentile("p99".to_owned(), 99.0));
            tpercentiles.push(Percentile("max".to_owned(), 100.0));

            snapshot.set_timing_percentiles(tkey.clone(), h1, &tpercentiles);

            let min_tpercentile = snapshot.timing_percentile(&tkey, tpercentiles[0].clone());
            let p50_tpercentile = snapshot.timing_percentile(&tkey, tpercentiles[1].clone());
            let p99_tpercentile = snapshot.timing_percentile(&tkey, tpercentiles[2].clone());
            let max_tpercentile = snapshot.timing_percentile(&tkey, tpercentiles[3].clone());
            let fake_tpercentile = snapshot.timing_percentile(&tkey, Percentile("fake".to_owned(), 63.0));

            assert!(min_tpercentile.is_some());
            assert!(p50_tpercentile.is_some());
            assert!(p99_tpercentile.is_some());
            assert!(max_tpercentile.is_some());
            assert!(fake_tpercentile.is_none());
        }

        {
            let mut h2 = Histogram::<u64>::new_with_bounds(1, u64::max_value(), 3).unwrap();
            h2.saturating_record(500_000);
            h2.saturating_record(750_000);
            h2.saturating_record(1_000_000);
            h2.saturating_record(1_250_000);

            let vkey = "ok".to_owned();
            let mut vpercentiles = Vec::new();
            vpercentiles.push(Percentile("min".to_owned(), 0.0));
            vpercentiles.push(Percentile("p50".to_owned(), 50.0));
            vpercentiles.push(Percentile("p99".to_owned(), 99.0));
            vpercentiles.push(Percentile("max".to_owned(), 100.0));

            snapshot.set_value_percentiles(vkey.clone(), h2, &vpercentiles);

            let min_vpercentile = snapshot.value_percentile(&vkey, vpercentiles[0].clone());
            let p50_vpercentile = snapshot.value_percentile(&vkey, vpercentiles[1].clone());
            let p99_vpercentile = snapshot.value_percentile(&vkey, vpercentiles[2].clone());
            let max_vpercentile = snapshot.value_percentile(&vkey, vpercentiles[3].clone());
            let fake_vpercentile = snapshot.value_percentile(&vkey, Percentile("fake".to_owned(), 63.0));

            assert!(min_vpercentile.is_some());
            assert!(p50_vpercentile.is_some());
            assert!(p99_vpercentile.is_some());
            assert!(max_vpercentile.is_some());
            assert!(fake_vpercentile.is_none());

        }
    }
}
