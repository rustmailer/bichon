// `country-flag-icons` ships typings for its barrel modules only, not for
// the per-flag subpaths (`country-flag-icons/string/3x2/DE`). Each of those
// files is a single default export holding the flag's SVG markup.
declare module 'country-flag-icons/string/3x2/*' {
  const svg: string
  export default svg
}
