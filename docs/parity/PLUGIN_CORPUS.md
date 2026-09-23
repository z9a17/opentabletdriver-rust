# Initial plugin corpus

`plugin-corpus.json` records the 57 catalog metadata entries eligible for OpenTabletDriver 0.6.7.0 at Plugin-Repository commit `2dfdff1cd77d274eb19c4d359b240167537c4db2`, grouped into 50 identities by name, owner and repository URL. Eligibility follows the pinned upstream inventory generator and says only that catalog version metadata permits the baseline.

The corpus copies the exact release URL, declared archive SHA-256, declared SPDX license identifier and supported driver-version range from catalog metadata. It also records the catalog metadata file's SHA-256 and Git path. The pinned source inventory stores the metadata SHA-256 for every catalog path; the validator compares every corpus row, path and copied field against that inventory. The two audited releases, Radial Follow 0.3.0.0 and BezierInterpolator 0.3.1.0, have archive hashes verified and managed metadata recorded in [plugin-archive-audit.json](plugin-archive-audit.json). The public types decorated with `PluginNameAttribute` are recorded as plugin candidates. Radial Follow declares two `IPositionedPipelineElement<IDeviceReport>` candidates; BezierInterpolator derives from `AsyncPositionedPipelineElement<IDeviceReport>`. These are metadata shape findings, not runtime discovery results. Neither plugin's code was loaded or executed. Other records remain metadata-only with unknown runtime categories, classes, assemblies, platform requirements, API dependencies, license conclusions and compatibility.

Validate the pinned corpus with:

```powershell
py -3 scripts/parity-plugin-corpus.py
py -3 scripts/parity-plugin-corpus.py --catalog-root <clean-pinned-Plugin-Repository-checkout>
py -3 -m unittest scripts.tests.test_parity_plugin_corpus
```

The validator checks schema version, all 57 records and 50 grouped identities, exact source paths and copied catalog fields, duplicate identity/version pairs, explicit classification states, declared counts, and consistency between versioned archive evidence and the corpus. Every `verified` archive hash or `managed_metadata_only` inspection claim requires a matching record in the companion audit file; such claims fail validation if the file is missing. `--catalog-root` additionally requires a clean checkout at the exact pinned commit and verifies each metadata file's SHA-256 against the corpus. The checked-in upstream inventory does not store metadata-file hashes, so this explicit checkout check is needed for byte-level source verification. Tests exercise baseline counts, duplicate/missing-state/path/hash failures, missing audit evidence, and archive-evidence consistency.

To extend this inventory, add evidence fields only with their source and verification method. Keep catalog claims separate from direct observations; after downloading an archive, compare its bytes to `archive_sha256_declared` and record the computed hash, retrieval time and exact URL. Inspect assemblies without loading or executing them where possible; record class/category findings with file hashes and tool/version provenance. Unknown or platform-dependent results stay as explicit blockers. Add manually installed examples in a separate section so they cannot change the pinned catalog counts. Do not commit downloaded plugin binaries unless redistribution is justified and licensed.
