/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{vue,js,ts}'],
  theme: {
    extend: {
      colors: {
        // 影音应用的主色：整体压暗，让海报和画面成为最亮的东西。
        // 不用纯黑 —— 纯黑背景下海报边缘会糊成一片。
        ink: {
          950: '#0b0d12',
          900: '#11141b',
          850: '#161a23',
          800: '#1c212c',
          700: '#252b38',
          600: '#343c4d',
          500: '#4d5566',
        },
        brand: {
          DEFAULT: '#5b8cff',
          soft: '#7aa2ff',
        },
      },
      fontFamily: {
        sans: [
          'system-ui',
          '-apple-system',
          'Segoe UI',
          'Noto Sans SC',
          'PingFang SC',
          'Microsoft YaHei',
          'sans-serif',
        ],
      },
    },
  },
  plugins: [],
}
