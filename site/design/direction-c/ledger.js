// Direction C: diff lines in the hero ledger write themselves in once the card is seen.
// Without JS (or with reduced motion) every line is simply visible.
const ledger = document.querySelector(".ledger");
if (ledger && "IntersectionObserver" in window && !matchMedia("(prefers-reduced-motion: reduce)").matches) {
  document.documentElement.classList.add("js");
  ledger.querySelectorAll(".diff__body span").forEach((line, k) => line.style.setProperty("--k", k));
  const io = new IntersectionObserver((entries) => {
    if (entries.some((e) => e.isIntersecting)) {
      ledger.classList.add("is-in");
      io.disconnect();
    }
  }, { threshold: 0.3 });
  io.observe(ledger);
}
