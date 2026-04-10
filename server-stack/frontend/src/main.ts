import { refreshDashboard } from "./views/dashboard.js";
import { loadNginx, saveNginx } from "./views/nginx.js";

void refreshDashboard();
setInterval(() => {
  void refreshDashboard();
}, 10_000);

document.getElementById("nginx-load")?.addEventListener("click", () => {
  void loadNginx();
});
document.getElementById("nginx-save")?.addEventListener("click", () => {
  void saveNginx();
});

void loadNginx();
