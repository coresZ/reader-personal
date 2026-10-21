import { createRouter, createWebHashHistory } from 'vue-router'
import { isWebUiMode } from '../api/transport'

const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    {
      path: '/',
      name: 'home',
      component: () => import('../views/HomeView.vue'),
    },
    {
      path: '/reader',
      name: 'reader',
      component: () => import('../views/ReaderView.vue'),
    },
    {
      path: '/explore',
      name: 'explore',
      component: () => import('../views/ExploreView.vue'),
    },
    {
      path: '/recent',
      name: 'recent',
      component: () => import('../views/RecentView.vue'),
    },
    {
      path: '/stats',
      name: 'stats',
      component: () => import('../views/StatsView.vue'),
    },
    {
      path: '/rss',
      name: 'rss',
      component: () => import('../views/RssView.vue'),
    },
    {
      path: '/rss/manage',
      name: 'rss-manage',
      component: () => import('../views/RssManageView.vue'),
    },
    {
      path: '/rss/article',
      name: 'rss-article',
      component: () => import('../views/RssArticleView.vue'),
    },
  ],
})

/**
 * Web 端只保留书架、最近阅读与阅读页。
 *
 * 隐藏导航项还不够: 地址栏是用户可写的, 收藏夹里也可能存着旧链接。这些页面
 * 在浏览器里要么整页空(`explore`/`rss` 全靠没有 HTTP 路由的接口), 要么只有
 * 半页数据(`stats` 的按书明细), 与其让人对着空页面猜, 不如退回书架。
 */
const WEB_ALLOWED_ROUTES = new Set(['home', 'reader', 'recent'])

router.beforeEach((to) => {
  if (!isWebUiMode()) return true
  if (typeof to.name === 'string' && WEB_ALLOWED_ROUTES.has(to.name)) return true
  return { name: 'home' }
})

export default router
