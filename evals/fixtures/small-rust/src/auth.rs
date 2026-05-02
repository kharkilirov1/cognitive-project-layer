pub struct AuthClaims {
    pub subject: String,
    pub scope: String,
}

pub fn validate_token(token: &str) -> bool {
    token.starts_with("cpl_") && token.len() > 12
}

pub fn parse_claims(token: &str) -> Option<AuthClaims> {
    if validate_token(token) {
        Some(AuthClaims {
            subject: token.trim_start_matches("cpl_").to_string(),
            scope: "read:context".to_string(),
        })
    } else {
        None
    }
}
