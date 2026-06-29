//! Loopback multi-node simulation of the SWIM gossip transport.

use std::net::SocketAddr;
use std::time::Duration;

use pulsate_cluster::{Cluster, Config};

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().expect("valid loopback addr")
}

/// A configuration with tight timings so the failure detector and anti-entropy
/// converge within a few hundred milliseconds.
fn fast_config(seeds: Vec<SocketAddr>) -> Config {
    let mut config = Config::new(loopback());
    config.seeds = seeds;
    config.probe_interval = Duration::from_millis(50);
    config.probe_timeout = Duration::from_millis(20);
    config.suspicion_timeout = Duration::from_millis(150);
    config.indirect_probes = 2;
    config.gossip_fanout = 4;
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_nodes_converge_and_merge_counters() {
    let node_a = Cluster::spawn(fast_config(vec![])).await.expect("spawn a");
    let addr_a = node_a.local_addr();
    let node_b = Cluster::spawn(fast_config(vec![addr_a]))
        .await
        .expect("spawn b");
    let node_c = Cluster::spawn(fast_config(vec![addr_a]))
        .await
        .expect("spawn c");

    // Let membership gossip converge.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    node_a.incr(1);
    node_b.incr(2);
    node_c.incr(3);

    // Let the counter increments propagate.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    assert_eq!(node_a.members().len(), 3, "a sees 3 members");
    assert_eq!(node_b.members().len(), 3, "b sees 3 members");
    assert_eq!(node_c.members().len(), 3, "c sees 3 members");

    let expected_leader = [node_a.node_id(), node_b.node_id(), node_c.node_id()]
        .into_iter()
        .min()
        .expect("a leader");
    assert_eq!(node_a.leader(), Some(expected_leader.clone()));
    assert_eq!(node_b.leader(), Some(expected_leader.clone()));
    assert_eq!(node_c.leader(), Some(expected_leader));

    assert_eq!(node_a.counter_value(), 6, "a merged counter");
    assert_eq!(node_b.counter_value(), 6, "b merged counter");
    assert_eq!(node_c.counter_value(), 6, "c merged counter");

    node_a.shutdown().await;
    node_b.shutdown().await;
    node_c.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dead_node_is_evicted() {
    let node_a = Cluster::spawn(fast_config(vec![])).await.expect("spawn a");
    let addr_a = node_a.local_addr();
    let node_b = Cluster::spawn(fast_config(vec![addr_a]))
        .await
        .expect("spawn b");
    let node_c = Cluster::spawn(fast_config(vec![addr_a]))
        .await
        .expect("spawn c");
    let id_c = node_c.node_id();

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(node_a.members().len(), 3, "converged to 3 first");

    // Kill c: dropping aborts its runtime and closes the socket, so it stops
    // acking and the survivors must detect the failure.
    drop(node_c);

    tokio::time::sleep(Duration::from_secs(2)).await;

    assert_eq!(node_a.members().len(), 2, "a evicted the dead node");
    assert_eq!(node_b.members().len(), 2, "b evicted the dead node");
    assert!(!node_a.members().contains(&id_c), "a dropped c's id");
    assert!(!node_b.members().contains(&id_c), "b dropped c's id");

    node_a.shutdown().await;
    node_b.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_node_is_its_own_leader() {
    let node = Cluster::spawn(fast_config(vec![])).await.expect("spawn");
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(node.members().len(), 1, "only itself");
    assert!(node.is_leader(), "solo node leads");
    assert_eq!(node.leader(), Some(node.node_id()));

    node.shutdown().await;
}
