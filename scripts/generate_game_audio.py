"""Generate the local development sound pack with an existing ElevenLabs API key.

The key is read from ELEVENLABS_API_KEY or CounterSplat's ignored .env.local.
Run explicitly; existing files are not replaced and requests consume API credits.
"""
from pathlib import Path
import json
import os
import re
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / 'assets' / 'sound' / 'first-person'
SOUNDS = {
    'rifle-shot.mp3': ('A single close, punchy AK-pattern rifle shot, sharp attack with short natural indoor decay, no voices or music, one shot only.', 1.2),
    'rifle-reload.mp3': ('First-person rifle magazine reload: one magazine release and insertion followed by a short metallic charging handle, dry detailed foley, no gunshot or voice.', 2.0),
    'dry-fire.mp3': ('Single dry metallic rifle trigger click, close and crisp, no gunshot, no voice or music.', 0.6),
    'bullet-impact.mp3': ('Single bullet striking hard stone, short sharp crack and a tiny grit debris tail, no gunshot or voices.', 0.8),
    'footstep.mp3': ('One boot footstep on a hard stone floor, close, short and natural, no speech, no music, one step only.', 0.7),
}

def api_key():
    value = os.environ.get('ELEVENLABS_API_KEY', '').strip()
    if value:
        return value
    path = ROOT / '.env.local'
    if path.is_file():
        for line in path.read_text(encoding='utf-8-sig').splitlines():
            match = re.match(r'^\s*(?:export\s+)?ELEVENLABS_API_KEY\s*=\s*(.*?)\s*$', line)
            if match:
                return match.group(1).strip().strip('"').strip("'")
    raise SystemExit('No ElevenLabs API key found in the environment or .env.local.')

def main():
    key = api_key()
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for name, (prompt, duration) in SOUNDS.items():
        destination = OUTPUT / name
        if destination.is_file() and destination.stat().st_size:
            print(f'{name}: already present')
            continue
        body = json.dumps({'text': prompt, 'duration_seconds': duration, 'prompt_influence': 0.4,
                           'model_id': 'eleven_text_to_sound_v2'}).encode('utf-8')
        request = urllib.request.Request('https://api.elevenlabs.io/v1/sound-generation?output_format=mp3_44100_128',
            data=body, headers={'xi-api-key': key, 'Content-Type': 'application/json', 'Accept': 'audio/mpeg'}, method='POST')
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                content_type = response.headers.get_content_type()
                data = response.read(8 * 1024 * 1024 + 1)
                if content_type not in ('audio/mpeg', 'audio/mp3') or not data or len(data) > 8 * 1024 * 1024:
                    raise RuntimeError('The service returned invalid or oversized audio.')
        except urllib.error.HTTPError as error:
            raise SystemExit(f'{name}: ElevenLabs returned HTTP {error.code}; no audio was saved.') from None
        except urllib.error.URLError as error:
            raise SystemExit(f'{name}: ElevenLabs request could not connect ({type(error.reason).__name__}).') from None
        temporary = destination.with_suffix('.tmp')
        temporary.write_bytes(data)
        temporary.replace(destination)
        print(f'{name}: saved {len(data)} bytes')

if __name__ == '__main__':
    main()
