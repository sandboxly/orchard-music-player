(() => {
  // Highlight the visitor's platform and point the hero button at its download.
  // Phones and tablets get the plain "Download" link (Android reports Linux,
  // iPads can report MacIntel).
  const agent = (navigator.userAgent || '').toLowerCase();
  const mobile = navigator.userAgentData?.mobile || /android|iphone|ipad|ipod/.test(agent) || (navigator.maxTouchPoints > 1 && /mac/.test(agent));
  const ua = (navigator.userAgentData?.platform || navigator.platform || agent).toLowerCase();
  const os = mobile ? null : ua.includes('win') ? 'windows' : ua.includes('mac') ? 'macos' : ua.includes('linux') || ua.includes('x11') ? 'linux' : null;
  if (os) {
    const card = document.querySelector(`.dl[data-os="${os}"]`);
    card?.classList.add('active');
    const hero = document.querySelector('[data-hero-dl]');
    const primary = card?.querySelector('[data-primary]');
    if (hero && primary) {
      hero.href = primary.href;
      hero.querySelector('span').textContent = 'Download for ' + card.dataset.label;
    }
  }

  // Copy buttons.
  document.querySelectorAll('[data-copy]').forEach((b) => {
    b.addEventListener('click', async () => {
      const text = document.getElementById(b.dataset.copy).textContent.trim();
      try { await navigator.clipboard.writeText(text); } catch { return; }
      const label = b.innerHTML;
      b.classList.add('ok');
      b.innerHTML = '<i class="ph ph-check" aria-hidden="true"></i><span class="sr">Copied</span>';
      b.setAttribute('aria-label', 'Copied');
      setTimeout(() => { b.classList.remove('ok'); b.innerHTML = label; b.setAttribute('aria-label', 'Copy command'); }, 1600);
    });
  });

  // Screenshot lightbox.
  const box = document.getElementById('lightbox');
  if (box) {
    const img = box.querySelector('img');
    const chip = box.querySelector('.chip');
    document.querySelectorAll('.shot').forEach((s) => s.addEventListener('click', () => {
      const src = s.querySelector('img');
      img.src = src.currentSrc || src.src;
      img.alt = src.alt;
      chip.textContent = s.dataset.name || '';
      box.showModal();
    }));
    box.addEventListener('click', (e) => { if (e.target === box || e.target.closest('.close')) box.close(); });
  }
})();
