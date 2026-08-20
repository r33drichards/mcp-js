import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const mainSuffixes = ['', '.js', '.json', '.node', '/index.js', '/index.json', '/index.node'];
const packageSuffixes = ['.js', '.json', '.node'];

function nodeError(code, message, ErrorType = Error) {
    const error = new ErrorType(message);
    error.code = code;
    return error;
}

function isUrl(value) {
    return Boolean(
        value?.href && value.protocol && value.auth === undefined && value.path === undefined,
    );
}

function virtualFileExists(filename) {
    return __mcpV8VirtualFiles.has(pathToFileURL(filename).href);
}

function resolveBaseForError(base) {
    if (isUrl(base)) return fileURLToPath(base);
    if (typeof base !== 'string') {
        throw nodeError(
            'ERR_INVALID_ARG_TYPE',
            'The "base" argument must be of type string or an instance of URL.',
            TypeError,
        );
    }
    let baseUrl;
    try {
        baseUrl = new URL(base);
    } catch {
        throw nodeError('ERR_INVALID_URL', 'Invalid URL', TypeError);
    }
    return fileURLToPath(baseUrl);
}

export function legacyMainResolve(packageJsonUrl, packageConfig, base) {
    if (!isUrl(packageJsonUrl)) {
        throw nodeError(
            'ERR_INTERNAL_ASSERTION',
            'This is caused by either a bug in Node.js or incorrect usage of Node.js internals.',
        );
    }

    const packagePath = fileURLToPath(new URL('.', packageJsonUrl));
    const main = typeof packageConfig.main === 'string' ? packageConfig.main : null;
    let missingPath;

    if (main !== null) {
        const initialPath = path.resolve(packagePath, main);
        missingPath = initialPath;
        for (const suffix of mainSuffixes) {
            const candidate = initialPath + suffix;
            if (virtualFileExists(candidate)) return pathToFileURL(candidate);
        }
    }

    const packageIndex = path.resolve(packagePath, './index');
    for (const suffix of packageSuffixes) {
        const candidate = packageIndex + suffix;
        if (virtualFileExists(candidate)) return pathToFileURL(candidate);
    }

    missingPath ||= packageIndex + '.js';
    const importedFrom = resolveBaseForError(base);
    throw nodeError(
        'ERR_MODULE_NOT_FOUND',
        `Cannot find package '${missingPath}' imported from ${importedFrom}`,
    );
}

export default { legacyMainResolve };
