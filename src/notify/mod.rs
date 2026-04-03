use regex::Regex;

pub struct NotificationMatcher {
    patterns: Vec<Regex>,
}

impl NotificationMatcher {
    pub fn new(patterns: &[String]) -> Self {
        let patterns = patterns.iter().filter_map(|p| Regex::new(p).ok()).collect();
        Self { patterns }
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn check(&self, text: &str) -> Option<String> {
        for re in &self.patterns {
            if let Some(m) = re.find(text) {
                return Some(m.as_str().to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_matcher_matches_nothing() {
        let matcher = NotificationMatcher::new(&[]);
        assert!(matcher.is_empty());
        assert!(matcher.check("anything").is_none());
    }

    #[test]
    fn test_single_pattern_matching() {
        let matcher = NotificationMatcher::new(&["ERROR".to_string()]);
        assert!(!matcher.is_empty());
        assert_eq!(
            matcher.check("something ERROR happened"),
            Some("ERROR".to_string())
        );
    }

    #[test]
    fn test_multiple_patterns() {
        let matcher = NotificationMatcher::new(&[
            "ERROR".to_string(),
            "OOM".to_string(),
            "build complete".to_string(),
        ]);
        assert_eq!(
            matcher.check("build complete"),
            Some("build complete".to_string())
        );
        assert_eq!(matcher.check("OOM killed"), Some("OOM".to_string()));
        assert_eq!(matcher.check("fatal ERROR"), Some("ERROR".to_string()));
    }

    #[test]
    fn test_case_sensitivity() {
        let matcher = NotificationMatcher::new(&["ERROR".to_string()]);
        assert!(matcher.check("error").is_none());
        assert!(matcher.check("Error").is_none());
        assert_eq!(matcher.check("ERROR"), Some("ERROR".to_string()));
    }

    #[test]
    fn test_no_match_returns_none() {
        let matcher = NotificationMatcher::new(&["ERROR".to_string(), "WARN".to_string()]);
        assert!(matcher.check("all good here").is_none());
        assert!(matcher.check("").is_none());
    }

    #[test]
    fn test_invalid_regex_patterns_skipped() {
        let matcher = NotificationMatcher::new(&["[invalid".to_string(), "ERROR".to_string()]);
        // Invalid pattern skipped, valid one works
        assert!(!matcher.is_empty());
        assert_eq!(matcher.check("ERROR"), Some("ERROR".to_string()));
    }

    #[test]
    fn test_all_invalid_patterns() {
        let matcher = NotificationMatcher::new(&["[invalid".to_string(), "(unclosed".to_string()]);
        assert!(matcher.is_empty());
        assert!(matcher.check("anything").is_none());
    }

    #[test]
    fn test_regex_pattern() {
        let matcher = NotificationMatcher::new(&["ERROR|FATAL".to_string()]);
        assert_eq!(matcher.check("FATAL crash"), Some("FATAL".to_string()));
        assert_eq!(matcher.check("ERROR occurred"), Some("ERROR".to_string()));
    }
}
