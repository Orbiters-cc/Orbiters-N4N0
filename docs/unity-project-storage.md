# Unity project storage recovery

The Unity projects panel checks the selected project's cache disk and the current Unity Editor log on a worker. A disk-space error must name that project's path before it is attributed to the project. What the panel says depends on what is true:

- **This disk is running low**: under 8 GiB free, and Unity has not complained. Nothing is stopped; the project opens as usual.
- **Unity ran out of disk space**: under 8 GiB free, and Unity's log says it could not write this project's files.
- **Unity ran out of disk space earlier**: the log says so, but the disk has room now. There is nothing to move; opening the project again lets it finish.

In the first two the panel measures the Library on a worker and says what **Repair storage** would do before it is pressed: how much it copies, how many files, to which folder, how much room is there. The button stays disabled until that is known, and is not offered when the Library is already redirected, holds a link, or no other disk has room; the panel then says what stands in the way.

## Opening

**Open in Unity never starts a long operation.** With a Library that exists, a low disk simply opens and the panel's warning stays. Only a Library that barely exists (none yet, or within 32 MiB and 256 files and folders) is moved by itself, where a move costs a moment and gives the first import a disk with room; the check stops walking at those limits, so it costs the same for a 40 GB Library. A problem with that move does not stop Unity opening. Opening used to run the whole repair without saying so: on a 40 GB Library with 7.2 GB free, hours of copying under "Opening Unity…", with no progress and no way to stop it.

Opening ends as soon as the editor has started: a program that cannot be started, or that quits at once with a failure, is reported, and Unity already holding the project has its window brought to the front instead of a second one being started. It gives up after 30 s on a disk that does not answer, saying so, so that the rest of the page is not left waiting.

## Repair

Quit that project's Unity window before repairing. Orbiters takes the Unity project lock for the repair; other Unity projects remain open, and Unity cannot open this project until the repair ends.

Repair copies the generated Library to `Orbiters/UnityCache/<id>` on a fixed local disk other than the Library's own, with room for the existing cache plus 8 GiB, checked again when it starts. It prefers the system drive's local application-data directory, then the eligible disk with the most room. It verifies every copied file byte for byte before switching Library to an NTFS junction. Assets, Packages, ProjectSettings and the project registration stay in place. Repairing does not open Unity afterwards: it can end hours later.

It runs on its own worker, not as one of the page's short actions: the gallery and other projects stay usable. The panel shows the bytes and files copied and checked out of the total, a progress bar and the time left; from elsewhere in the page a capsule over the cards shows the same.

**Cancel** works until the copy is checked. Cancelling, or any failure (a disk filling, a file unreadable, something holding Library so that it cannot be renamed, a failed redirect), leaves the original Library as it was and deletes the partial copy; the panel reports what happened, and the project stays in the gallery. Once Library is switched the move is done: the old copy is deleted with its progress shown, and if that fails the move still stands and the leftovers go the next time the window opens.

No SDK upgrades, manifest rewrites or unrelated cache deletion are part of recovery. If the Library already has a redirect whose disk is full, or no suitable disk exists, Orbiters reports that space must be freed instead of changing an unknown redirect. The 8 GiB allowance is a reserve, not a guarantee that every project import will fit.

## What an interrupted repair leaves

Closing the window or killing the app stops a repair where it is (a repair runs in the window's process; the background service goes on without it). Each time the window opens, and before a repair, a worker looks at every `Orbiters/UnityCache/<id>` on the fixed disks and removes those no Library points at, with their `.json`, and puts back a Library a kill left renamed. A cache is kept whenever that cannot be proved:

- the project its `.json` names has a Library link there, or any project the gallery has found does (a project moved by hand takes its junction along), whether or not that project is in the gallery;
- that project's disk is not connected, or its folder is not there (it may have been moved);
- the `.json` is missing or unreadable, or a Library link cannot be read;
- the project's Unity lock is held: Unity has it open, or a repair is running now. It is looked at again next time.

Only folders named like a cache (32 hex digits) are ever considered.

The cache's destination disk must remain available while using the project. This repair addresses lack of space for Library imports; it does not repair unrelated package-resolution errors or expand the source disk for new assets.
