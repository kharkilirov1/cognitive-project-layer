#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub user_id: String,
    pub issued_by: &'static str,
}

pub fn create_session(user_id: &str) -> Session {
    Session {
        user_id: user_id.to_string(),
        issued_by: "cognitive-project-layer",
    }
}
