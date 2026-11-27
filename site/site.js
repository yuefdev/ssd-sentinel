(() => {
  const root = document.documentElement;
  const saved = localStorage.getItem("ssd-sentinel-site-theme");
  if (saved === "light" || saved === "dark") {
    root.setAttribute("data-theme", saved);
  }

  const themeBtn = document.getElementById("theme-toggle");
  const syncThemeLabel = () => {
    if (!themeBtn) return;
    const isLight = root.getAttribute("data-theme") === "light";
    themeBtn.textContent = isLight ? "Dark Theme" : "Light Theme";
    themeBtn.setAttribute("aria-pressed", String(isLight));
  };
  syncThemeLabel();

  if (themeBtn) {
    themeBtn.addEventListener("click", () => {
      const next = root.getAttribute("data-theme") === "light" ? "dark" : "light";
      root.setAttribute("data-theme", next);
      localStorage.setItem("ssd-sentinel-site-theme", next);
      syncThemeLabel();
    });
  }

  const navToggle = document.getElementById("nav-toggle");
  const navLinks = document.getElementById("site-nav");
  if (navToggle && navLinks) {
    navToggle.addEventListener("click", () => {
      const open = navLinks.classList.toggle("open");
      navToggle.setAttribute("aria-expanded", String(open));
    });
  }

  document.querySelectorAll("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const targetId = btn.getAttribute("data-copy");
      const codeEl = targetId ? document.getElementById(targetId) : null;
      if (!codeEl) return;
      try {
        await navigator.clipboard.writeText(codeEl.textContent || "");
        const prev = btn.textContent;
        btn.textContent = "Copied";
        setTimeout(() => {
          btn.textContent = prev;
        }, 1600);
      } catch {
        btn.textContent = "Copy manually";
      }
    });
  });
})();
