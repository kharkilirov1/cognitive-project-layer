import { validateToken } from "./auth";

interface LoginRequest {
  body: {
    token?: string;
  };
}

interface LoginResponse {
  status(code: number): LoginResponse;
  json(payload: unknown): void;
}

export function loginHandler(req: LoginRequest, res: LoginResponse) {
  const result = validateToken(req.body.token ?? "");
  if (!result) {
    return res.status(401).json({ error: "invalid_token" });
  }

  return res.json({
    sessionId: `session:${result.userId}`,
    scope: result.scope,
  });
}

export function registerLoginRoute(app: { post: Function }) {
  app.post("/login", loginHandler);
}
