// Copy only the selected source, preserving its original whitespace.
for (const button of document.querySelectorAll('[data-copy]')) {
  button.addEventListener('click', async () => {
    const target = document.getElementById(button.dataset.copy);
    const source = target.querySelector('code')?.textContent ?? target.textContent;
    try {
      await navigator.clipboard.writeText(source);
      button.textContent = 'Copied';
      document.getElementById('copy-status').textContent = 'Source copied to clipboard.';
    } catch {
      button.textContent = 'Select source';
      const range = document.createRange();
      range.selectNodeContents(target);
      const selection = window.getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      document.getElementById('copy-status').textContent = 'Source selected. Use your keyboard to copy it.';
    }
    setTimeout(() => { button.textContent = 'Copy source'; }, 2200);
  });
}
