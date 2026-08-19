import { SiteHeader } from '@/components/qw/site-header'
import { Hero } from '@/components/qw/hero'
import { Architecture } from '@/components/qw/architecture'
import { Faq } from '@/components/qw/faq'
import { Footer } from '@/components/qw/footer'

export default function Page() {
  return (
    <div id="top" className="min-h-screen bg-background">
      <SiteHeader />
      <main>
        <Hero />
        <Architecture />
        <Faq />
      </main>
      <Footer />
    </div>
  )
}
