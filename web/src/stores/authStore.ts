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

import { LoginResult } from "@/api/users/api";


export const WEBUITOKEN = 'f4d3e92d7b1241a8a0b3e7cdb5c5d19d';

export interface StoredToken {
  accessToken: string;
}

export const setToken = (result: LoginResult) => {
  if (!result.success || !result.access_token) {
    console.error("Invalid login result");
    return;
  }

  const data: StoredToken = {
    accessToken: result.access_token,
  };

  localStorage.setItem(WEBUITOKEN, JSON.stringify(data));
};


export const updateToken = (newToken: string) => {
  const item = localStorage.getItem(WEBUITOKEN);
  if (!item) return;

  try {
    const data: StoredToken = JSON.parse(item);
    data.accessToken = newToken;
    localStorage.setItem(WEBUITOKEN, JSON.stringify(data));
  } catch (error) {
    console.error("Error updating access token:", error);
    resetToken();
  }
};

export const getToken = (): StoredToken | null => {
  const item = localStorage.getItem(WEBUITOKEN);
  if (!item) return null;

  try {
    return JSON.parse(item) as StoredToken;
  } catch (error) {
    console.error("Error parsing access token:", error);
    resetToken();
    return null;
  }
};

export const resetToken = () => {
  localStorage.removeItem(WEBUITOKEN);
};