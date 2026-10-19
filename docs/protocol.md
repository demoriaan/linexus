# USB protocol notes

These notes describe how Linexus talks to the Corsair iCUE Nexus, as implemented in `crates/nexus-core`. They were derived for interoperability by observing the device and are not official documentation. They cover only what this implementation uses; byte values without a known meaning are listed as the values that are sent.

## Device

| Property | Value |
|---|---|
| USB vendor ID | `0x1b1c` |
| USB product ID | `0x1b8e` |
| Display | 640 x 48 pixels |
| Interface used | The HID interface with usage page `0x000c`. It takes images, control commands and reports touches. |

The device exposes several HID interfaces. When more than one matches the vendor ID, product ID and usage page, the implementation takes the one with the lowest device path. On Linux it is opened through hidraw (`/dev/hidrawN`).

## Report overview

| Report ID | Direction | HID report type | Purpose |
|---|---|---|---|
| `0x01` | device to host | input | Touch position |
| `0x02` | host to device | output | Image data |
| `0x03` | host to device | feature (set) | Brightness, blank, built-in animation |
| `0x05` | device to host | feature (get) | Device information, firmware version |

In the byte tables below, offset 0 is the report ID.

## Frame upload

A frame is 640 x 48 pixels with 4 bytes per pixel, 122,880 bytes in total, row by row from the top-left corner. It is sent as a series of output reports with ID `0x02`. Every report is 1024 bytes long: an 8-byte header and up to 1016 bytes of pixel data. A full frame takes 121 reports: 120 with 1016 bytes and a last one with 960 bytes.

| Offset | Size | Value |
|---|---|---|
| 0 | 1 | `0x02` (report ID) |
| 1 | 1 | `0x05` |
| 2 | 1 | `0x40` |
| 3 | 1 | `0x01` on the last report of a frame, otherwise `0x00` |
| 4 | 2 | Block number, little-endian, starting at 0 |
| 6 | 2 | Number of pixel bytes in this report, little-endian (1016, or 960 for the last) |
| 8 | up to 1016 | Pixel data; the rest of the 1024 bytes is zero |

### Pixel format

Each pixel is 4 bytes in the order **B, G, R, X**. The implementation sets X to `0xFF`.

The panel shows 18-bit colour: it ignores the two lowest bits of each channel. To keep gradients and anti-aliased edges from banding, the encoder adds a 4 x 4 ordered dither before sending. For the pixel at column `x` and row `y`, it takes the threshold from this matrix at row `y mod 4`, column `x mod 4`, divides it by 4 (integer division, giving 0 to 3) and adds the result to each colour channel, capped at 255:

```text
 0   8   2  10
12   4  14   6
 3  11   1   9
15   7  13   5
```

A black pixel therefore leaves the host with channel values between 0 and 3.

### Pacing

These figures describe this implementation, not limits of the device:

- One frame of HID writes takes about 15 ms. While something moves on the strip, the daemon sends a frame every 20 ms (about 50 frames per second).
- When nothing moves, it renders at least every 500 ms and sends a frame only when the pixels changed. An unchanged frame is sent again every 2 seconds.

## Control commands

Control commands are feature reports with ID `0x03`.

| Bytes | Command |
|---|---|
| `03 01 pp` | Set the backlight brightness. `pp` is 0 to 100; the host clamps larger values to 100. |
| `03 04` | Blank the screen |
| `03 0d ii ll` | Play built-in animation `ii`; `ll` is `01` to loop it (the implementation sends `00` otherwise) |
| `03 0f` | Stop the built-in animation |

How the daemon uses them:

- It sends `03 01 pp` whenever the effective brightness changes (profile brightness, idle dimming).
- To turn the strip off, it sends `03 04` and stops sending frames. To turn it on again, it resumes sending frames.
- When it stops, it sends `03 0d 01 01` and `03 01 64` (brightness 100), so the panel shows its own looping animation instead of the last frame.
- `03 0f` is defined in `nexus-core` but not sent by the daemon; sending frames replaces the animation.

## Device information

A get-feature-report request for ID `0x05` (64-byte buffer) returns device information. From byte 6 on, it holds the firmware version as an ASCII string ending with a zero byte, for example `2.4.438`. `linexusctl info` prints it.

## Touch input

The strip sends input reports with ID `0x01`. The implementation reads at most 64 bytes per report and uses the first 8:

| Offset | Value |
|---|---|
| 0 | `0x01` (report ID) |
| 1 | `0x02` |
| 2 | `0x21` |
| 3, 4 | Not used |
| 5 | Contact: non-zero while a finger touches the strip |
| 6, 7 | X position, little-endian, 0 to 639 |

- Reports shorter than 8 bytes, or with other values in bytes 0 to 2, are ignored.
- An X value of 640 or more is not used as a position. If such a report lifts the finger, the touch ends at the last position on the panel; a touch whose first reports are off the panel starts with its first valid one.
- There is no Y position in what this implementation reads, so widgets cover the full strip height and gestures are horizontal only.
- The implementation tracks a single contact. The touch phase follows from the contact byte of two consecutive reports: 0 then non-zero starts a touch, non-zero then non-zero moves it, non-zero then 0 ends it.

The daemon waits for touch reports between frames, at most `--touch-poll-ms` (default 50 ms), and handles up to 64 reports per pass so a resting finger cannot block rendering. Gesture recognition (tap, double tap, hold, swipe) happens on the host; see [architecture.md](architecture.md#from-touch-to-action).

## Trying it out

`linexusctl` has commands that talk to the strip directly. They take the same lock as the daemon, so stop the daemon first (`systemctl --user stop linexusd.service`).

| Command | Effect |
|---|---|
| `linexusctl info` | Read the firmware version |
| `linexusctl brightness <percent>` | Send `03 01 pp` |
| `linexusctl blank` | Send `03 04` |
| `linexusctl demo --pattern rgb-bars` | Upload a test frame: red, green and blue thirds |
| `linexusctl demo --pattern border-grid` | Upload a 1-pixel border with a 100 x 12 pixel grid |
| `linexusctl image <file> --fit fill` | Upload a picture scaled to 640 x 48 (`fill`, `fit` or `stretch`) |
| `linexusctl touch [seconds]` | Print touch events and recognized gestures, 10 seconds by default |
