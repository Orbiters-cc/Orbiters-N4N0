# Menu input reliability — 30 October 2026

The menu could lose a pointer at valid hand positions, discard a trigger pull during an incomplete contact
reading, and retain a held press after the trigger had been released over a submenu.

`menu/touch.rs` now solves the four contact surface-distance equations directly. Subtracting their squares gives
the sender centre as a linear function of its radius; a quadratic supplies the candidate radii. The existing
sender-radius and residual limits still reject inconsistent readings. The previous Gauss–Newton iteration could
diverge or find the wrong root near the screen's plane. A regression traverses 100 hand positions and three targets
per position; the previous implementation lost `[-0.6, 0.5, -0.1]` pointing at the centre even with ideal readings.

A trigger edge during an incomplete reading waits for a valid aim within the existing 100 ms loss grace. Release,
expiry, or recovery outside the page cancels it. A held trigger cannot click on re-entry. ScreenRuntime clears its
held flag on Hover as well as Away, and closes the input session when the menu closes. Submenu navigation keeps the
physical press latched until release, so one touch still cannot click through successive pages.

The avatar placement fix is in Orbiters Toolkit's `OrbInstaller.Placement.cs`, delivered separately as an N4N0
development gallery update. A completed glide or released grip settles first. Visibility must remain valid for
200 ms before it can arm the existing 400 ms look-away timer. This prevents short contact pulses from restarting
recenter indefinitely. Closing the screen drops its handle grip even before the hand gesture relaxes.

Validation: Rust unit and integration tests cover positioning, gaze, touch approach/release, loss recovery,
trigger cancellation, submenu navigation, music dragging, and close/reopen. Unity tests use the SDK's actual
constraint solver with parameter-driver Set behaviours simulated on animator entry. Injecting one visible frame
per 600 ms reproduced nine recenter restarts in six seconds before the fix.

The automated tests do not reproduce a running VRChat client or controller hardware. After updating the desktop
and the N4N0 gallery item and uploading the avatar, check both hands, repeated submenu/back clicks, touch and ray
input, moving around the screen, grabbing/releasing, closing while grabbed, looking away and back, and screen sizes
at both ends. A stable centred menu should remain still, a sustained look-away should still recenter it, and a held
press should never activate another page until released.
