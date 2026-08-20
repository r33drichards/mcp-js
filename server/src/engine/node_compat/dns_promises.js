// node:dns/promises - the exact promises namespace exported by node:dns.

import { promises } from 'node:dns';

export default promises;
export const {
    Resolver, lookup, lookupService, resolveTxt, resolveSrv, resolve4, resolve6,
    NODATA, FORMERR, SERVFAIL, NOTFOUND, NOTIMP, REFUSED,
    BADQUERY, BADNAME, BADFAMILY, BADRESP, CONNREFUSED, TIMEOUT,
    EOF, FILE, NOMEM, DESTRUCTION, BADSTR, BADFLAGS, NONAME,
    BADHINTS, NOTINITIALIZED, LOADIPHLPAPI, ADDRGETNETWORKPARAMS,
    CANCELLED, ADDRCONFIG, V4MAPPED, ALL,
} = promises;
