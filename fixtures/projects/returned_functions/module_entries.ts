import { notify, wrap } from "./module_wrappers";

export const enabled = wrap(notify, true);
export const disabled = wrap(notify, false);
export default wrap(notify, true);

// @expect MW1001 line=3 function=enabled
// @expect MW1001 line=5 function=default
