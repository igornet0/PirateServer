import express from "express";
import path from "path";
import { fileURLToPath } from "url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const app = express();
const port = Number(process.env.PORT) || 3000;

app.use(express.static(__dirname));

app.get("/", (_req, res) => {
  res.sendFile(path.join(__dirname, "index.html"));
});

// 0.0.0.0 — нужно для Docker (test-local / деплой), иначе слушается только localhost в контейнере.
app.listen(port, "0.0.0.0", () => {
  console.log(`test-project listening on 0.0.0.0:${port}`);
});
