//
// Copyright (c) 2025-2026 rustmailer.com (https://rustmailer.com)
//
// This file is part of the Bichon Email Archiving Project
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.


import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'


import en from './locales/en.json'
import zh from './locales/zh.json'
import ar from './locales/ar.json'
import de from './locales/de.json'
import es from './locales/es.json'
import fi from './locales/fi.json'
import fr from './locales/fr.json'
import it from './locales/it.json'
import jp from './locales/jp.json'
import ko from './locales/ko.json'
import nl from './locales/nl.json'
import pt from './locales/pt.json'
import pl from './locales/pl.json'
import ru from './locales/ru.json'
import da from './locales/da.json'
import sv from './locales/sv.json'
import no from './locales/no.json'
import cs from './locales/cs.json'
import zh_tw from './locales/zh-tw.json'


const getSavedLanguage = () => {
  if (typeof window !== 'undefined' && window.localStorage) {
    return localStorage.getItem('i18nextLng') || 'en'
  }
  return 'en'
}

const savedLanguage = getSavedLanguage()

i18n
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: en },
      zh: { translation: zh },
      ar: { translation: ar },
      cs: { translation: cs },
      da: { translation: da },
      de: { translation: de },
      es: { translation: es },
      fi: { translation: fi },
      fr: { translation: fr },
      it: { translation: it },
      jp: { translation: jp },
      ko: { translation: ko },
      nl: { translation: nl },
      no: { translation: no },
      pl: { translation: pl },
      pt: { translation: pt },
      ru: { translation: ru },
      sv: { translation: sv },
      'zh-tw': { translation: zh_tw }
    },
    lng: savedLanguage,
    fallbackLng: 'en',
    interpolation: { escapeValue: false },
    lowerCaseLng: true
  })

i18n.on('languageChanged', (lng) => {
  if (typeof window !== 'undefined' && window.localStorage) {
    localStorage.setItem('i18nextLng', lng)
  }
})

export default i18n
