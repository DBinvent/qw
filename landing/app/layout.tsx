import type { Metadata, Viewport } from 'next'
import { Inter, JetBrains_Mono } from 'next/font/google'
import './globals.css'

const inter = Inter({
  subsets: ['latin'],
  variable: '--font-inter',
  display: 'swap',
})

const jetbrainsMono = JetBrains_Mono({
  subsets: ['latin'],
  variable: '--font-jetbrains-mono',
  display: 'swap',
})

export const metadata: Metadata = {
  title: 'QW — knownby.work',
  description:
    'Skills confirmed by the people you worked with. Found through friends of friends. A peer-verified contribution network — no blockchain, no tokens, no central authority.',
  metadataBase: new URL('https://knownby.work'),
  openGraph: {
    title: 'QW — knownby.work',
    description: 'Skills confirmed by the people you worked with. Found through friends of friends.',
    url: 'https://knownby.work',
    siteName: 'QW',
    type: 'website',
  },
}

export const viewport: Viewport = {
  colorScheme: 'dark',
  themeColor: '#09090b',
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang="en" className={`dark bg-background ${inter.variable} ${jetbrainsMono.variable}`}>
      <body className="font-sans antialiased">{children}</body>
    </html>
  )
}
