// node:fs/promises — node:fs's promises namespace under its own module
// name. Same posture as node:fs: import-compatible rejecting stubs (the
// sandbox's real filesystem surface is the policy-gated `fs` capability,
// globalThis.fs), served so `import { readFile } from 'node:fs/promises'`
// links and unused code paths load.

import { promises } from 'node:fs';

export default promises;
export const {
    access, appendFile, chmod, chown, copyFile, cp, glob,
    lchown, link, lstat, lutimes, mkdir, mkdtemp, open,
    opendir, readdir, readFile, readlink, realpath, rename,
    rm, rmdir, stat, statfs, symlink, truncate, unlink,
    utimes, watch, writeFile, constants,
} = promises;
