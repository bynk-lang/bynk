export type Result<T, E> =
  | { readonly tag: "Ok"; readonly value: T }
  | { readonly tag: "Err"; readonly error: E };

export const Ok = <T>(value: T): Result<T, never> => ({ tag: "Ok", value });
export const Err = <E>(error: E): Result<never, E> => ({ tag: "Err", error });

export type Option<T> =
  | { readonly tag: "Some"; readonly value: T }
  | { readonly tag: "None" };

export const Some = <T>(value: T): Option<T> => ({ tag: "Some", value });
export const None: Option<never> = { tag: "None" };

