/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./templates/**/*.html",
    "./static/ts/**/*.ts",
  ],
  theme: {
    borderRadius: {
      none: '0',
      DEFAULT: '0',
      full: '9999px',
    },
    extend: {
      colors: {
        sw: {
          deep: '#06080c',
          dark: '#0c0f16',
          bg: '#141820',
          light: '#1c2230',
          lighter: '#2a3144',
          fg: '#e5e0d8',
          dim: '#8b8680',
          muted: '#5c5852',
          amber: '#f0a830',
          'amber-dim': '#c48820',
          red: '#e54d2e',
          teal: '#40c0a0',
          blue: '#6ca8d0',
          cream: '#faf0dc',
        },
      },
      fontFamily: {
        mono: ["'Space Mono'", 'ui-monospace', 'monospace'],
        sans: ["'Outfit'", 'system-ui', 'sans-serif'],
      },
    },
  },
  plugins: [],
};
