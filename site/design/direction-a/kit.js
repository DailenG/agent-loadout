// Copy buttons for install commands. Progressive enhancement only.
for (const button of document.querySelectorAll("[data-copy]")) {
  button.addEventListener("click", async () => {
    const label = button.textContent;
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      button.textContent = "Copied";
    } catch {
      button.textContent = "Select";
    }
    setTimeout(() => { button.textContent = label; }, 1600);
  });
}
