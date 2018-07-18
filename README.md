# hotmic

[![conduct-badge][]][conduct] [![travis-badge][]][travis] [![downloads-badge][] ![release-badge][]][crate] [![license-badge][]](#license)

[conduct-badge]: https://img.shields.io/badge/%E2%9D%A4-code%20of%20conduct-blue.svg
[travis-badge]: https://img.shields.io/travis/nuclearfurnace/hotmic/master.svg
[downloads-badge]: https://img.shields.io/crates/d/hotmic.svg
[release-badge]: https://img.shields.io/crates/v/hotmic.svg
[license-badge]: https://img.shields.io/crates/l/hotmic.svg
[conduct]: https://github.com/nuclearfurnace/hotmic/blob/master/CODE_OF_CONDUCT.md
[travis]: https://travis-ci.org/nuclearfurnace/hotmic
[crate]: https://crates.io/crates/hotmic

__hotmic__ is a high-speed metrics collection library, based on [crossbeam-channe]() and [mio]().  It is shameless fork of [tic](https://github.com/brayniac/tic) with some internal changes to support `crossbeam-channel` and to fit my needs better.  This project would not be possible without `tic`!

## code of conduct

**NOTE**: All conversations and contributions to this project shall adhere to the [Code of Conduct][conduct].

## usage

The API documentation of this library can be found at [docs.rs/hotmic](https://docs.rs/hotmic/).

## general features
- based on `crossbeam-channel`/`mio`, so it's blazingly fast (faster than `tic`; see rough numbers [here](#performance))
- supports counters, gauges, and histograms
- provides dynamic faceting: what portion of metric data should be recorded, and in what way
- control mechanism to allow any caller to retrieve metric snapshots at any time

## performance

Like `tic`, performance is the name of the game for `hotmic`.  It was a primary concern!  As the metrics library for a high-speed caching layer load balancer ([synchrotron](https://github.com/nuclearfurnace/synchrotron)), low overhead and low latency is important important important.

Out of the gate, `tic` itself is very fast.  Fast enough that unless you're counting micros, you almost certainly wouldn't need this much speed:

    # target/release/examples/benchmark --batch 128 --capacity 128 --windows 30
    2018-07-17 22:45:06 INFO  [tic benchmark] rate: 10272865.422551299 samples per second
    2018-07-17 22:45:06 INFO  [tic benchmark] latency (ns): p50: 111 p90: 117 p999: 2274 p9999: 9421 max: 25048384
    2018-07-17 22:45:06 INFO  [tic benchmark] total metrics pushed: 304100737

Let's take a look at __hotmic__!

    # RUST_LOG=info target/release/examples/benchmark --batch 128 --capacity 128 --duration 30
    INFO 2018-07-18T02:54:19Z: benchmark: rate: 16397009.186308714 samples per second
    INFO 2018-07-18T02:54:19Z: benchmark: latency (ns): p50: 43 p90: 53 p99: 4787 p999: 5339 max: 30367
    INFO 2018-07-18T02:54:20Z: benchmark: total metrics pushed: 476316288

Over 50% more throughput _and_ lower latency across most percentiles.  Now, there are some caveats here:

- `tic` allocates at runtime in the critical path (when there are no free buffers to reuse) instead of blocking
- `hotmic` opts to bound its runtime memory consumption by pre-allocating all buffers and blocking until one returns
- both benchmarks are using themselves to measure themselves, so, mistakes can happen!
- `hotmic` is, in fact, doing less than `tic` does in terms of metric support, no doubt about it

It's a little lop-sided... both libraries have varying levels of metric types available, have different dependencies, but otherwise parallel goals: be simple, be fast.  While running consistent benchmarks is itself a labor unto itself, I've modeled `hotmic` after `tic`, insofar as having a matching benchmark example, so you can attempt to repeat the results for yourself.

__NOTE__: One area where `hotmic` falls down compared to `tic` is at batch sizes of 1.  Every `Source` (or `Sender` in `tic`) has a batch size they abide by, batching up samples before sending them off to be aggregated/processed.  If you want the most real-time values, you would naturally choose a batch size of 1: every metric sample is immediately sent off.

In this case, `hotmic` is much slower because of its bounded memory approach.  As `tic` will freely allocate new buffers if none are available in its free buffer list, it never waits or pauses, but `hotmic` does wait, and so throughput greatly suffers.  Here's an example of changing nothing besides batch size down to 1:

    INFO 2018-07-18T03:09:23Z: benchmark: rate: 500526.1117313251 samples per second
    INFO 2018-07-18T03:09:23Z: benchmark: latency (ns): p50: 3847 p90: 4091 p99: 7803 p999: 25887 max: 79871
    INFO 2018-07-18T03:09:24Z: benchmark: total metrics pushed: 14498450

Woof!  500k samples/sec is still nothing to sneeze at for a single instance of an application, but the latencies!  I'm not sure if I'll ever add in the ability to burst beyond the limits of the free buffer list, to match `tic`, and to match its performance.  Ultimately, these numbers are under full load -- a single thread sending metrics as fast as it can, to be precise -- and so in practice, blocking for a buffer may seldom occur... leaving latencies low if you stay under the throughput limit.

I plan to include proper histogram logs in the future, at least for `hotmic` (`tic doesn't use `HdrHistogram`, `hotmic` does), at varying request rates for some common batch/capacity values.  This should be more informative on what performance you can expect in your own application based on expected workload.

(hotmic 3743d224bc10ae3808033acb68a91703b972fbd6, tic d77b3c615ff13ad89ba2b081e73a2f70e68428d9, July 2018)

## license

Per the flexible `tic` licensing terms, __hotmic__ is released solely under the MIT license. ([LICENSE](LICENSE) or http://opensource.org/licenses/MIT)

Attribution information for `tic` can be found in the same license file.
