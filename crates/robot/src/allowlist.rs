//! The allowlist IS the subscribe set — nothing outside it is ever read. See docs/wiki/robot.md

/// Whether `topic` is covered: exact names, or one trailing `/*` wildcard.
#[must_use]
pub fn topic_allowed(allowlist: &[String], topic: &str) -> bool {
    allowlist.iter().any(|entry| {
        entry.strip_suffix("/*").map_or_else(
            || entry == topic,
            |prefix| topic == prefix || topic.starts_with(&format!("{prefix}/")),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[&str]) -> Vec<String> {
        entries.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn empty_allowlist_serves_nothing() {
        assert!(!topic_allowed(&[], "/rover/odom"));
    }

    #[test]
    fn exact_match_only_that_topic() {
        let l = list(&["/rover/odom"]);
        assert!(topic_allowed(&l, "/rover/odom"));
        assert!(!topic_allowed(&l, "/rover/odom2"));
        assert!(!topic_allowed(&l, "/rover/cmd_vel"));
    }

    #[test]
    fn wildcard_covers_subtree_not_siblings() {
        let l = list(&["/rover/sensors/*"]);
        assert!(topic_allowed(&l, "/rover/sensors/imu"));
        assert!(topic_allowed(&l, "/rover/sensors/lidar/points"));
        assert!(topic_allowed(&l, "/rover/sensors"));
        assert!(!topic_allowed(&l, "/rover/sensorsX"));
        assert!(!topic_allowed(&l, "/rover/cmd_vel"));
    }
}
