(function () {
  var modes = ['nullradix', 'light', 'dark', 'system'];
  var mode = 'nullradix';
  try {
    mode = localStorage.getItem('nullradix-theme') || mode;
  } catch (e) {}
  var look = null;
  if (modes.indexOf(mode) < 0) {
    try {
      look = JSON.parse(localStorage.getItem('nullradix-theme-installed') || 'null');
    } catch (e) {}
    if (!look || look.id !== mode || typeof look.styleUrl !== 'string') {
      look = null;
      mode = 'nullradix';
    }
  }
  var prefersDark = window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
  // safe mode: an installed theme shows light or dark by the system, without its CSS
  var safe = look !== null && new URLSearchParams(location.search).has('safe-theme');
  if (safe) look = null;
  var dark =
    mode === 'nullradix' || mode === 'dark' || ((mode === 'system' || safe) && prefersDark);
  if (look) dark = look.scheme === 'dark';
  document.documentElement.dataset.theme = dark ? 'dark' : 'light';
  document.documentElement.dataset.themeMode = mode;
  if (look) {
    // comes before the app stylesheet here; applyInstalledTheme moves it after it on mount
    var link = document.createElement('link');
    link.id = 'nullradix-theme-css';
    link.rel = 'stylesheet';
    link.href = look.styleUrl;
    document.head.appendChild(link);
    if (look.startColor) {
      document.documentElement.dataset.startBg = '';
      document.documentElement.style.setProperty('--start-bg', look.startColor);
    }
  }
})();
