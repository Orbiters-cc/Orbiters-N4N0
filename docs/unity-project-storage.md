# Unity project storage recovery

The Unity projects panel checks the selected project's cache disk and the current Unity Editor log on a worker. A disk-space error must name that project's path before it is attributed to the project. When less than 8 GiB is available for the project's Library, the panel offers **Repair storage and open Unity**.

Quit that project's Unity window before repairing. Orbiters takes the Unity project lock for the repair; other Unity projects remain open.

Repair copies the generated Library to `Orbiters/UnityCache/<id>` on a fixed local disk with room for the existing cache plus 8 GiB. It prefers the system drive's local application-data directory, then another eligible local disk. It verifies every copied file byte for byte before switching Library to an NTFS junction. Assets, Packages, ProjectSettings and the project registration stay in place. A failed directory switch restores the original Library. A failed copy leaves the original intact and reports the partial-copy location.

Opening a project through Orbiters performs this storage preflight automatically. No SDK upgrades, manifest rewrites or unrelated cache deletion are part of recovery. If the Library already has a redirect whose disk is full, or no suitable disk exists, Orbiters reports that space must be freed instead of changing an unknown redirect. The 8 GiB allowance is a reserve, not a guarantee that every project import will fit.

The cache's destination disk must remain available while using the project. This repair addresses lack of space for Library imports; it does not repair unrelated package-resolution errors or expand the source disk for new assets.
