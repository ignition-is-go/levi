//! Bucketed set-hashing for the hub sync leg (spec: "Merkle-style
//! comparison is a later optimization" — this is it). Event ids are grouped
//! by their 2-hex prefix (the same sharding as the events ref) and each
//! bucket is summarized as the SHA-256 of its sorted ids. Equal hash ⇒
//! identical bucket ⇒ skip; differing buckets are enumerated id-by-id.
//! Both the CLI and the hub compute this identically.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

/// bucket ("ab") → hex SHA-256 of the bucket's sorted ids.
pub fn bucket_hashes<'a>(ids: impl Iterator<Item = &'a str>) -> BTreeMap<String, String> {
    let mut buckets: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for id in ids {
        if id.len() < 2 {
            continue;
        }
        buckets.entry(id[..2].to_string()).or_default().push(id);
    }
    buckets
        .into_iter()
        .map(|(bucket, mut ids)| {
            ids.sort_unstable();
            let mut hasher = Sha256::new();
            for id in ids {
                hasher.update(id.as_bytes());
                hasher.update(b"\n");
            }
            (bucket, format!("{:x}", hasher.finalize()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_order_independent() {
        let a = bucket_hashes(["abcd", "abff", "cd00"].into_iter());
        let b = bucket_hashes(["cd00", "abff", "abcd"].into_iter());
        assert_eq!(a, b);
        assert_eq!(a.len(), 2);
        let c = bucket_hashes(["abcd", "cd00"].into_iter());
        assert_ne!(a.get("ab"), c.get("ab"));
        assert_eq!(a.get("cd"), c.get("cd"));
    }
}
