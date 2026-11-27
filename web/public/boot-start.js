(function () {
  var box = document.getElementById('start-anim');
  var root = document.getElementById('root');
  if (!box || !root) return;
  var seen = false;
  try {
    seen = sessionStorage.getItem('nullradix-started') === '1';
    sessionStorage.setItem('nullradix-started', '1');
  } catch (e) {}
  var still = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (seen || still) {
    box.remove();
    return;
  }
  root.inert = true;
  setTimeout(function () {
    root.inert = false;
    if (box.isConnected) box.remove();
  }, 5000);
})();
