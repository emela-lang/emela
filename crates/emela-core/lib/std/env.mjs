// Host functions for Env on JS. Reads `process.env`.

// Whether the environment variable named `name` is set. `hasOwn`, so that names on the prototype
// (`toString`) do not count.
export const has = (name) => Object.hasOwn(process.env, name);

// The value of the environment variable named `name`. Call only when `has` is true.
export const get = (name) => process.env[name];
