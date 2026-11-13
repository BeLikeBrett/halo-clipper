# Advanced editing behavior

Changes in 0.9 preserve the app's existing theme and live overlapping preview.
Simple Clip keeps its mark-and-export workflow.

## Reversible attachment

A media group stores member clip IDs and whether the timeline displays it as a
compound bar. Attachment does not merge samples, change source ranges, fill gaps,
restore deleted clips, reset gain/speed, or change track order. Playback/export
always consume the original underlying clips. Undo snapshots include groups.

New video placements start attached to their audio streams. Detaching exposes
independent pieces. Splits made while detached retain their remembered group;
reattaching the video restores that group, including edited audio pieces. Several
selected videos and audio pieces can be attached together. Splitting an attached
compound produces independent left/right compounds, including at an internal gap.
Moving a compound applies the same time delta to all its members.

## Canvas shape and placement

The largest video placed on the timeline supplies the default source dimensions.
Unplaced imports do not affect it. Aspect modes use that source's short edge;
for example, 3840 × 2160 becomes 2160 × 3840 in vertical mode. No numeric resolution
picker appears in the preview. Custom export settings can override output size.

Fit preserves the whole frame. Fill covers the canvas and crops overflow. Zoom,
position, opacity, and canvas background apply in preview and export. Blur fills
the canvas from the lowest visible video at that moment; it falls back to the
next video through gaps, and to black where no video is active. It is independent
of foreground placement. The live GPU blur approximates FFmpeg's Gaussian blur.

Official editor references informing these choices:

- [Final Cut Pro: spatial conform](https://support.apple.com/en-ae/guide/final-cut-pro/ver3363b44e/mac)
  separates Fit/Fill behavior from project aspect ratio.
- [Adobe Premiere: Auto Reframe overview](https://helpx.adobe.com/premiere/desktop/add-video-effects/commonly-used-effects/auto-reframe-overview.html)
  treats sequence shape separately from individual framing. Halo's framing is
  manual; this change does not add subject tracking.
- [CapCut: portrait/landscape conversion](https://www.capcut.com/resource/convert-portrait-videos-to-landscape)
  combines ratio selection with a canvas background/blur option.

## Validation

Model tests cover default combined bars, repeated attach/detach after cuts,
deletions, gaps, speed/gain edits, multiple sources and external audio, undo,
moving compounds, and splitting through internal gaps. Real FFmpeg tests verify
portrait output dimensions, Fit/Fill, zoom, solid/blurred backgrounds, empty gaps,
video ordering/opacity, and independent audio timing/gain/speed. Thumbnail tests
extract actual decoded frames. Native UI checks use a separate Xvfb display and
isolated configuration/cache directories.
