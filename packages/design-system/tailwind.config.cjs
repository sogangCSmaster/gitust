/** @type {import('tailwindcss').Config} */
module.exports = {
  darkMode: ["class", '[data-theme="dark"]'],
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      minHeight: { touch: "44px" },
      minWidth: { touch: "44px" },
      fontSize: { sm: ["13px", { lineHeight: "1.25rem" }] },
      colors: Object.fromEntries(
        [
          "border",
          "input",
          "ring",
          "background",
          "foreground",
          "primary",
          "secondary",
          "destructive",
          "muted",
          "accent",
          "popover",
          "card",
          "success",
          "warning",
          "error",
          "info",
          "sidebar",
        ].map((name) => [
          name,
          name === "border" ||
          name === "input" ||
          name === "ring" ||
          name === "background" ||
          name === "foreground"
            ? `hsl(var(--${name}) / <alpha-value>)`
            : {
                DEFAULT: `hsl(var(--${name}) / <alpha-value>)`,
                foreground: `hsl(var(--${name}-foreground) / <alpha-value>)`,
                ...(name === "primary"
                  ? { hover: "hsl(var(--primary-hover) / <alpha-value>)" }
                  : {}),
                ...(name === "sidebar"
                  ? {
                      accent: "hsl(var(--sidebar-accent))",
                      "accent-foreground":
                        "hsl(var(--sidebar-accent-foreground))",
                      border: "hsl(var(--border))",
                      ring: "hsl(var(--ring))",
                    }
                  : {}),
              },
        ]),
      ),
      borderRadius: {
        lg: "var(--radius)",
        md: "calc(var(--radius) - 2px)",
        sm: "calc(var(--radius) - 4px)",
      },
      fontFamily: {
        sans: ["-apple-system", "BlinkMacSystemFont", "Segoe UI", "sans-serif"],
        mono: ["SFMono-Regular", "Consolas", "Liberation Mono", "monospace"],
      },
    },
  },
  plugins: [require("tailwindcss-animate")],
};
