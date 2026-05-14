import type { Metadata } from "next";
import { Nav } from "@/components/nav";
import { Footer } from "@/components/footer";
import { locales, type Locale } from "@/lib/i18n/config";
import { getSiteUrl } from "@/lib/site";

export function generateStaticParams() {
  return locales.map((locale) => ({ locale }));
}

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }): Promise<Metadata> {
  const { locale } = await params;
  const isZh = locale === "zh";
  const siteUrl = getSiteUrl();
  return {
    title: isZh ? "Writer · 长篇小说工作台" : "Writer · Long-form fiction workspace",
    description: isZh
      ? "基于 DeepSeek V4 的本地优先长篇小说创作工作台。支持故事工程、章节创作、审稿、修订和连续性记忆。"
      : "Terminal-native long-form fiction workspace for DeepSeek V4. Draft community site for installation, docs, roadmap, and release notes.",
    metadataBase: new URL(siteUrl),
    openGraph: {
      title: isZh ? "Writer · 长篇小说工作台" : "Writer",
      description: isZh
        ? "基于 DeepSeek V4 的本地优先长篇小说创作工作台。"
        : "Terminal-native long-form fiction workspace for DeepSeek V4.",
      url: siteUrl,
      siteName: "Writer",
      type: "website",
    },
    twitter: { card: "summary_large_image" },
    alternates: {
      languages: {
        en: "/en",
        zh: "/zh",
      },
    },
  };
}

export default async function LocaleLayout({
  children,
  params,
}: {
  children: React.ReactNode;
  params: Promise<{ locale: string }>;
}) {
  const { locale } = await params;

  return (
    <>
      <Nav locale={locale as Locale} />
      <main>{children}</main>
      <Footer locale={locale as Locale} />
    </>
  );
}
