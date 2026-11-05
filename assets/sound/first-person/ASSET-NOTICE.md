# Weapon sound placeholders

The five `.mp3` clips here are silent placeholders (MPEG frames of digital
silence) so the gameplay scene's audio layers load in the browser demo. They
contain no recorded or generated audio; replace them with real clips when
those are made.

Likewise `assets/images/first-person/playback/raising/0001.webp`, the one footage
frame the scene preloads, is a 1x1 pure-green placeholder: the weapon compositor's
green key removes it entirely, so a raised weapon with no footage draws nothing
(only the crosshair and ammo count). The gameplay scene spawns with the weapon
stowed (`WeaponConfig.start_stowed`, toggled with F) until new frames exist.
