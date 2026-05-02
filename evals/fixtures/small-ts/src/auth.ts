export interface AuthResult {
  userId: string;
  scope: string;
}

export function validateToken(token: string): AuthResult | null {
  if (!token.startsWith("cpl_")) {
    return null;
  }

  return {
    userId: token.slice(4),
    scope: "read:context",
  };
}
