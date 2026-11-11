// Not an SDK client: a project object that only looks like one.
export const resend = { emails: { send: async (message: unknown) => message } };
// @expect none
