import { registerLoginRoute } from "./routes";

export function bootstrapServer(app: { post: Function }) {
  registerLoginRoute(app);
}
