# subar

This is a relatively basic status bar for [sway](https://swaywm.org/) and [i3](https://i3wm.org/). It's not intended to be a serious project that works for lots of use cases, but perhaps others will find it useful.

## Features

- Australian weather from [BOM Buddy](https://github.com/sublipri/bom-buddy)
- Now playing from [MPD](https://www.musicpd.org/)
- Current volume from WirePlumber
- Current date and time

## Usage

The `MPD_HOST` environment variable is read if set. The `--no-stop-on-hide` flag prevents the process from being suspended when the bar is hidden. Features can be disabled with the `--no-mpd`, `--no-vol`, and `--no-bom` flags. If using the weather feature, you must either pass `--check-weather` or run `bom-buddy monitor` separately.
