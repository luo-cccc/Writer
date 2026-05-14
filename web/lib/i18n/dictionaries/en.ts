/** en dictionary — minimal, pages carry inline copy */
const en = {
  nav: {
    links: [
      { href: "/install", label: "Install", cn: "安装" },
      { href: "/docs", label: "Docs", cn: "文档" },
      { href: "/feed", label: "Activity", cn: "动态" },
      { href: "/roadmap", label: "Roadmap", cn: "路线" },
      { href: "/contribute", label: "Contribute", cn: "参与" },
    ],
    edition: "Edition",
    online: "API · Online",
    install: "Install →",
    starGitHub: "★ GitHub",
  },
  footer: {
    cols: [
      {
        title: "Product",
        cn: "产品",
        items: [
          { label: "Install", href: "/install" },
          { label: "Documentation", href: "/docs" },
          { label: "Roadmap", href: "/roadmap" },
          { label: "Releases", href: "https://github.com/luo-cccc/Writer/releases" },
        ],
      },
      {
        title: "Community",
        cn: "社区",
        items: [
          { label: "Issues", href: "https://github.com/luo-cccc/Writer/issues" },
          { label: "Pull Requests", href: "https://github.com/luo-cccc/Writer/pulls" },
          { label: "Discussions", href: "https://github.com/luo-cccc/Writer/discussions" },
          { label: "Contribute", href: "/contribute" },
        ],
      },
      {
        title: "Resources",
        cn: "资源",
        items: [
          { label: "Activity Feed", href: "/feed" },
          { label: "Code of Conduct", href: "https://github.com/luo-cccc/Writer/blob/main/CODE_OF_CONDUCT.md" },
          { label: "Security", href: "https://github.com/luo-cccc/Writer/blob/main/SECURITY.md" },
          { label: "License (MIT)", href: "https://github.com/luo-cccc/Writer/blob/main/LICENSE" },
        ],
      },
    ],
    tagline:
      "Open-source terminal-native long-form fiction workspace built on DeepSeek V4. MIT licensed. Pull requests welcome.",
    crafted: "Made with care · 用心制作",
    poweredBy: "本网站由 DeepSeek V4-Flash 协同维护",
    mirrors: "镜像源 / Mirror",
  },
  localeSwitch: { en: "EN", zh: "中文" },
};

export default en;
