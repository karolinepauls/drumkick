drumkick
========

Practice metronome/drum machine for musicians and amateurs who want more than a preset.

Features
--------

- TUI or headless mode (with ``--headless``). Both support pausing and BPM changes.
- Samples included in the binary (``CLICK``, ``click``, ``DRIP``, ``drip``)
- Sample loading from files
- Any beat count supported
- Any beat subdivision (want quintuplets - that's as simple as ``1/5``)
- Formats: mp3, ogg (vorbis), flac, wav, pcm

**Unsupported but planned:**

- shifting meters and looping

- polyrhythms (measures played in parallel)

- BPM changes

- "human" timing feel

Examples
--------

A pattern with samples.

.. code:: sh

    drumkick-cli '1:kick,hat and:kick,hat 2:hat,snare and:hat 3:hat,kick and:hat 4:hat,snare and:hat' \
        --bpm 80 \
        -s kick="../avl-drumkits/Samples/36-Ludwig26Kick-3.flac" \
        -s snare="../avl-drumkits/Samples/38-PearlSnare2-2.flac" \
        -s hat="../avl-drumkits/Samples/42-SabianRockHatClosed-1.flac"

Get samples from https://github.com/studiorack/avl-drumkits/

Patterns
~~~~~~~~

- **even**: ``'1:click'``

- **4/4**: ``'1:CLICK 2:click 3:click 4:click'``

- **6/8**: ``'1:DRIP 2:click 3:click 4:CLICK 5:click 6:click'``

- **4/4 quarter note triplets**: ``'1:DRIP trip:click let:click 2:CLICK trip:click let:click 3:CLICK trip:click let:click 4:CLICK trip:click let:click'``

- **quintuplets**: ``'1:CLICK 1/5:click 2/5:click 3/5:click 4/5:click'``

- **4/4 internal clock practice pattern (half bar filled, half empty)**: ``'1:CLICK and:click 2:CLICK and:click 3: 4:'``
