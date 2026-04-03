use super::{Session, SessionState};

pub struct SessionManager {
    pub sessions: Vec<Session>,
    pub active_index: Option<usize>,
    pub last_active_index: Option<usize>,
    max_sessions: usize,
}

impl SessionManager {
    pub fn new(max_sessions: usize) -> Self {
        Self {
            sessions: Vec::new(),
            active_index: None,
            last_active_index: None,
            max_sessions,
        }
    }

    pub fn add_session(&mut self, session: Session) -> Result<usize, String> {
        if self.sessions.len() >= self.max_sessions {
            return Err(format!(
                "Maximum {} concurrent sessions reached",
                self.max_sessions
            ));
        }
        let idx = self.sessions.len();
        self.sessions.push(session);
        self.last_active_index = self.active_index;
        self.active_index = Some(idx);
        Ok(idx)
    }

    pub fn remove_session(&mut self, index: usize) -> Option<Session> {
        if index >= self.sessions.len() {
            return None;
        }
        let session = self.sessions.remove(index);

        // Fix active_index
        match self.active_index {
            Some(active) if active == index => {
                if self.sessions.is_empty() {
                    self.active_index = None;
                } else if active >= self.sessions.len() {
                    self.active_index = Some(self.sessions.len() - 1);
                }
            }
            Some(active) if active > index => {
                self.active_index = Some(active - 1);
            }
            _ => {}
        }

        // Fix last_active_index
        match self.last_active_index {
            Some(last) if last == index => self.last_active_index = None,
            Some(last) if last > index => self.last_active_index = Some(last - 1),
            _ => {}
        }

        Some(session)
    }

    pub fn active_session(&self) -> Option<&Session> {
        self.active_index.and_then(|i| self.sessions.get(i))
    }

    #[allow(dead_code)]
    pub fn active_session_mut(&mut self) -> Option<&mut Session> {
        self.active_index.and_then(|i| self.sessions.get_mut(i))
    }

    pub fn switch_to(&mut self, index: usize) -> bool {
        if index < self.sessions.len() {
            self.last_active_index = self.active_index;
            self.active_index = Some(index);
            if let Some(session) = self.sessions.get_mut(index) {
                session.has_new_output = false;
            }
            true
        } else {
            false
        }
    }

    pub fn switch_next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let next = match self.active_index {
            Some(i) => (i + 1) % self.sessions.len(),
            None => 0,
        };
        self.switch_to(next);
    }

    pub fn switch_prev(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let prev = match self.active_index {
            Some(0) => self.sessions.len() - 1,
            Some(i) => i - 1,
            None => 0,
        };
        self.switch_to(prev);
    }

    pub fn switch_last(&mut self) {
        if let Some(last) = self.last_active_index {
            self.switch_to(last);
        }
    }

    pub fn has_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }

    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    #[allow(dead_code)]
    pub fn active_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| matches!(s.state, SessionState::Active | SessionState::Suspended))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(label: &str) -> Session {
        Session::new(
            uuid::Uuid::new_v4().to_string(),
            label.to_string(),
            "host.test".to_string(),
            22,
            "user".to_string(),
            1000,
        )
    }

    #[test]
    fn test_add_and_switch() {
        let mut mgr = SessionManager::new(9);
        let idx = mgr.add_session(make_session("s1")).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(mgr.active_index, Some(0));

        let idx2 = mgr.add_session(make_session("s2")).unwrap();
        assert_eq!(idx2, 1);
        assert_eq!(mgr.active_index, Some(1));

        mgr.switch_to(0);
        assert_eq!(mgr.active_index, Some(0));
        assert_eq!(mgr.last_active_index, Some(1));
    }

    #[test]
    fn test_max_sessions() {
        let mut mgr = SessionManager::new(2);
        mgr.add_session(make_session("s1")).unwrap();
        mgr.add_session(make_session("s2")).unwrap();
        let result = mgr.add_session(make_session("s3"));
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_session() {
        let mut mgr = SessionManager::new(9);
        mgr.add_session(make_session("s1")).unwrap();
        mgr.add_session(make_session("s2")).unwrap();
        mgr.add_session(make_session("s3")).unwrap();
        mgr.switch_to(1);

        let removed = mgr.remove_session(0);
        assert!(removed.is_some());
        assert_eq!(mgr.sessions.len(), 2);
        assert_eq!(mgr.active_index, Some(0)); // shifted down
    }

    #[test]
    fn test_switch_next_prev() {
        let mut mgr = SessionManager::new(9);
        mgr.add_session(make_session("s1")).unwrap();
        mgr.add_session(make_session("s2")).unwrap();
        mgr.add_session(make_session("s3")).unwrap();
        mgr.switch_to(0);

        mgr.switch_next();
        assert_eq!(mgr.active_index, Some(1));

        mgr.switch_next();
        assert_eq!(mgr.active_index, Some(2));

        mgr.switch_next();
        assert_eq!(mgr.active_index, Some(0)); // wraps

        mgr.switch_prev();
        assert_eq!(mgr.active_index, Some(2)); // wraps back
    }

    #[test]
    fn test_switch_last() {
        let mut mgr = SessionManager::new(9);
        mgr.add_session(make_session("s1")).unwrap();
        mgr.add_session(make_session("s2")).unwrap();
        mgr.switch_to(0);
        mgr.switch_to(1);

        mgr.switch_last();
        assert_eq!(mgr.active_index, Some(0));
    }
}
