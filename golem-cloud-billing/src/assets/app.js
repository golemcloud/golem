for (const button of document.querySelectorAll("button[data-copy]")) {
  button.hidden = false
  button.addEventListener("click", async () => {
    const source = document.getElementById(button.dataset.copy)
    if (source === null) return
    try {
      await navigator.clipboard.writeText(source.textContent ?? "")
      button.textContent = "Copied"
    } catch {
      button.textContent = "Copy failed"
    }
    setTimeout(() => {
      button.textContent = "Copy"
    }, 2000)
  })
}
