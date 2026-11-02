const status = document.getElementById('launch-status');
const button = document.getElementById('launch-game');
try {
  const response = await fetch(new URL('./availability.json', import.meta.url), {cache:'no-store'});
  if (!response.ok) throw new Error('Availability is missing');
  const info = await response.json();
  if (info.available === true) {
    status.textContent = 'Opening the live WebGPU canvas\u2026';
    button.hidden = false;
    window.location.replace(new URL('./play/', import.meta.url).href);
  } else {
    status.textContent = 'The playable build is being prepared. Check back once a level and the game assets are available.';
  }
} catch {
  status.textContent = 'The playable build is not available here yet. Return to the project page to learn more.';
}
