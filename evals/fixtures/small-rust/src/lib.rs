pub mod auth;
pub mod session;

pub fn login_flow(token: &str, user_id: &str) -> Option<session::Session> {
    if auth::validate_token(token) {
        Some(session::create_session(user_id))
    } else {
        None
    }
}
