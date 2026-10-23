/** Headless local adaptation: unchanged pinned server + narrower loopback bind.
 * Full production desktop/Xvnc/nut.js image is a separate Compose profile.
 */
import http from 'node:http';
import { pathToFileURL } from 'node:url';
const source=process.argv[2];
if(!source)throw new Error('Pass the pinned computer-use images/browser/browser/server.js path');
const listen=http.Server.prototype.listen;
http.Server.prototype.listen=function(...args){if(args[1]==='::')args[1]='127.0.0.1';return listen.apply(this,args)};
await import(pathToFileURL(source).href);
