export const START_OVERLAY_ID = 'start-anim';

export const START_OVERLAY_FADE_MS = 300;

// Fades out the start overlay from index.html and gives the app back to input.
export function dismissStartOverlay(doc: Document = document): void {
  doc.getElementById('root')?.removeAttribute('inert');
  const overlay = doc.getElementById(START_OVERLAY_ID);
  if (!overlay) return;
  overlay.classList.add('is-done');
  setTimeout(() => {
    overlay.remove();
  }, START_OVERLAY_FADE_MS);
}
